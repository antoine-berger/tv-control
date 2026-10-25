use super::{clean_path, fail, DIRECTORY};
use std::{
    collections::BTreeMap,
    ffi::CString,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
};
use tv_domain::PortResult;

#[derive(Clone, Copy, PartialEq, Eq)]
struct Key {
    dev: u64,
    ino: u64,
}
impl Key {
    fn of(metadata: &fs::Metadata) -> Self {
        Self {
            dev: metadata.dev(),
            ino: metadata.ino(),
        }
    }
}
fn private_file(file: &File) -> PortResult<Key> {
    let m = file.metadata().map_err(|_| fail("supervision_store_io"))?;
    if !m.is_file()
        || m.uid() != unsafe { libc::geteuid() }
        || m.mode() & 0o7777 != 0o600
        || m.nlink() != 1
    {
        return Err(fail("supervision_store_unsafe"));
    }
    Ok(Key::of(&m))
}
fn private_directory(path: &Path) -> PortResult<Key> {
    if !clean_path(path) {
        return Err(fail("supervision_store_configuration"));
    }
    for ancestor in path.ancestors() {
        let m = ancestor.symlink_metadata().map_err(|e| {
            fail(if e.kind() == std::io::ErrorKind::NotFound {
                "supervision_store_missing"
            } else {
                "supervision_store_io"
            })
        })?;
        if !m.is_dir() || m.file_type().is_symlink() {
            return Err(fail("supervision_store_unsafe"));
        }
    }
    let m = path
        .symlink_metadata()
        .map_err(|_| fail("supervision_store_io"))?;
    if m.uid() != unsafe { libc::geteuid() } || m.mode() & 0o7777 != 0o700 {
        return Err(fail("supervision_store_unsafe"));
    }
    Ok(Key::of(&m))
}
fn open_at(directory: &File, name: &str, create: bool) -> PortResult<File> {
    let name = CString::new(name).map_err(|_| fail("supervision_store_configuration"))?;
    let flags = libc::O_RDWR
        | libc::O_CLOEXEC
        | libc::O_NOFOLLOW
        | libc::O_NONBLOCK
        | if create {
            libc::O_CREAT | libc::O_EXCL
        } else {
            0
        };
    let fd = unsafe { libc::openat(directory.as_raw_fd(), name.as_ptr(), flags, 0o600) };
    if fd < 0 {
        let code = match std::io::Error::last_os_error().kind() {
            std::io::ErrorKind::NotFound => "supervision_store_missing",
            std::io::ErrorKind::AlreadyExists => "supervision_store_exists",
            _ => "supervision_store_io",
        };
        return Err(fail(code));
    }
    let file = unsafe { File::from_raw_fd(fd) };
    private_file(&file)?;
    Ok(file)
}

pub(super) struct FileLock {
    file: File,
    directory: Key,
}
impl Drop for FileLock {
    fn drop(&mut self) {
        unsafe {
            libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
        }
    }
}
pub(super) struct Directory {
    connection: PathBuf,
    path: PathBuf,
    file: File,
    key: Key,
    locks: Mutex<BTreeMap<String, Key>>,
}
impl Directory {
    pub(super) fn prepare(connection: &Path) -> PortResult<Self> {
        private_directory(connection)?;
        let path = connection.join(DIRECTORY);
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&path)
            .map_err(|e| {
                fail(if e.kind() == std::io::ErrorKind::AlreadyExists {
                    "supervision_store_exists"
                } else {
                    "supervision_store_io"
                })
            })?;
        Self::open_directory(connection)
    }
    pub(super) fn open(connection: &Path) -> PortResult<Self> {
        let directory = Self::open_directory(connection)?;
        let mut pins = directory
            .locks
            .lock()
            .map_err(|_| fail("supervision_store_io"))?;
        for name in [super::TRANSACTION, super::OWNER] {
            let file = open_at(&directory.file, name, false)?;
            pins.insert(name.to_owned(), private_file(&file)?);
        }
        drop(pins);
        Ok(directory)
    }
    fn open_directory(connection: &Path) -> PortResult<Self> {
        private_directory(connection)?;
        let path = connection.join(DIRECTORY);
        let key = private_directory(&path)?;
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_DIRECTORY)
            .open(&path)
            .map_err(|_| fail("supervision_store_io"))?;
        if Key::of(&file.metadata().map_err(|_| fail("supervision_store_io"))?) != key {
            return Err(fail("supervision_store_changed"));
        }
        Ok(Self {
            connection: connection.into(),
            path,
            file,
            key,
            locks: Mutex::new(BTreeMap::new()),
        })
    }
    pub(super) fn path(&self) -> &Path {
        &self.path
    }
    pub(super) fn check(&self) -> PortResult<()> {
        private_directory(&self.connection)?;
        if private_directory(&self.path)? != self.key {
            return Err(fail("supervision_store_changed"));
        }
        Ok(())
    }
    pub(super) fn create(&self, name: &str, bytes: &[u8]) -> PortResult<()> {
        self.check()?;
        let mut file = open_at(&self.file, name, true)?;
        file.write_all(bytes)
            .and_then(|_| file.sync_all())
            .map_err(|_| fail("supervision_store_io"))?;
        self.file
            .sync_all()
            .map_err(|_| fail("supervision_store_io"))?;
        if [super::TRANSACTION, super::OWNER].contains(&name) {
            self.locks
                .lock()
                .map_err(|_| fail("supervision_store_io"))?
                .insert(name.to_owned(), private_file(&file)?);
        }
        Ok(())
    }
    pub(super) fn read(&self, name: &str, limit: usize) -> PortResult<Vec<u8>> {
        self.check()?;
        let file = open_at(&self.file, name, false)?;
        let mut bytes = Vec::new();
        file.take(limit as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| fail("supervision_store_io"))?;
        if bytes.len() > limit {
            return Err(fail("supervision_store_limit"));
        }
        Ok(bytes)
    }
    pub(super) fn lock(&self, name: &str, busy: &'static str) -> PortResult<FileLock> {
        self.check()?;
        let file = open_at(&self.file, name, false)?;
        let key = private_file(&file)?;
        let pinned = self
            .locks
            .lock()
            .map_err(|_| fail("supervision_store_io"))?
            .get(name)
            .copied();
        if pinned != Some(key) {
            return Err(fail("supervision_store_changed"));
        }
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(fail(match std::io::Error::last_os_error().raw_os_error() {
                Some(n) if n == libc::EWOULDBLOCK || n == libc::EAGAIN => busy,
                _ => "supervision_store_lock_failed",
            }));
        }
        let guard = FileLock {
            file,
            directory: self.key,
        };
        if private_file(&open_at(&self.file, name, false)?)? != key {
            return Err(fail("supervision_store_changed"));
        }
        Ok(guard)
    }
    fn check_lock(&self, lock: &FileLock, name: &str) -> PortResult<()> {
        self.check()?;
        if lock.directory != self.key {
            return Err(fail("supervision_owner_mismatch"));
        }
        let key = private_file(&lock.file)?;
        let pinned = self
            .locks
            .lock()
            .map_err(|_| fail("supervision_store_io"))?
            .get(name)
            .copied();
        if pinned != Some(key) || private_file(&open_at(&self.file, name, false)?)? != key {
            return Err(fail("supervision_owner_mismatch"));
        }
        Ok(())
    }
    pub(super) fn check_owner(&self, owner: &FileLock) -> PortResult<()> {
        self.check_lock(owner, super::OWNER)
    }
    pub(super) fn purge(&self, owner: &FileLock, transaction: &FileLock) -> PortResult<()> {
        self.check_owner(owner)?;
        self.check_lock(transaction, super::TRANSACTION)?;
        let names = [super::OWNER, super::TRANSACTION, super::STATE];
        // Enumerate the pinned directory, not a pathname that might have been
        // replaced. fdopendir takes ownership of this separate CLOEXEC duplicate.
        let duplicate = unsafe { libc::fcntl(self.file.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 3) };
        if duplicate < 0 {
            return Err(fail("supervision_store_io"));
        }
        let entries = unsafe { libc::fdopendir(duplicate) };
        if entries.is_null() {
            unsafe {
                libc::close(duplicate);
            }
            return Err(fail("supervision_store_io"));
        }
        struct Entries(*mut libc::DIR);
        impl Drop for Entries {
            fn drop(&mut self) {
                unsafe {
                    libc::closedir(self.0);
                }
            }
        }
        let entries = Entries(entries);
        unsafe {
            libc::rewinddir(entries.0);
        }
        let mut observed = Vec::new();
        loop {
            // readdir distinguishes EOF from failure through thread-local errno.
            #[cfg(target_os = "macos")]
            unsafe {
                *libc::__error() = 0;
            }
            #[cfg(target_os = "linux")]
            unsafe {
                *libc::__errno_location() = 0;
            }
            let entry = unsafe { libc::readdir(entries.0) };
            if entry.is_null() {
                if std::io::Error::last_os_error().raw_os_error() != Some(0) {
                    return Err(fail("supervision_store_io"));
                }
                break;
            }
            let name = unsafe { std::ffi::CStr::from_ptr((*entry).d_name.as_ptr()) };
            if name == c"." || name == c".." {
                continue;
            }
            let name = name
                .to_str()
                .map_err(|_| fail("supervision_purge_foreign_entry"))?;
            if !names.contains(&name) || observed.iter().any(|v| v == name) {
                return Err(fail("supervision_purge_foreign_entry"));
            }
            observed.push(name.to_owned());
        }
        if observed.len() != names.len() {
            return Err(fail("supervision_store_changed"));
        }
        let mut files = Vec::new();
        for name in names {
            let file = open_at(&self.file, name, false)?;
            let key = private_file(&file)?;
            files.push((name, file, key));
        }
        self.check()?;
        let parent_key = private_directory(&self.connection)?;
        let parent = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_DIRECTORY)
            .open(&self.connection)
            .map_err(|_| fail("supervision_store_io"))?;
        if Key::of(
            &parent
                .metadata()
                .map_err(|_| fail("supervision_store_io"))?,
        ) != parent_key
        {
            return Err(fail("supervision_store_changed"));
        }
        let mut mutated = false;
        let result = (|| {
            for (name, file, key) in &files {
                self.check()?;
                if private_file(file)? != *key
                    || private_file(&open_at(&self.file, name, false)?)? != *key
                {
                    return Err(fail("supervision_store_changed"));
                }
                let name =
                    CString::new(*name).map_err(|_| fail("supervision_store_configuration"))?;
                if unsafe { libc::unlinkat(self.file.as_raw_fd(), name.as_ptr(), 0) } != 0 {
                    return Err(fail("supervision_store_io"));
                }
                mutated = true;
            }
            self.file
                .sync_all()
                .map_err(|_| fail("supervision_store_io"))?;
            self.check()?;
            if private_directory(&self.connection)? != parent_key {
                return Err(fail("supervision_store_changed"));
            }
            if unsafe {
                libc::unlinkat(
                    parent.as_raw_fd(),
                    c"supervision".as_ptr(),
                    libc::AT_REMOVEDIR,
                )
            } != 0
            {
                return Err(fail("supervision_store_io"));
            }
            parent.sync_all().map_err(|_| fail("supervision_store_io"))
        })();
        result.map_err(|error| {
            if mutated {
                error.after_dispatch()
            } else {
                error
            }
        })
    }
    pub(super) fn replace(&self, name: &str, bytes: &[u8]) -> PortResult<()> {
        self.check()?;
        let previous = open_at(&self.file, name, false)?;
        let previous_key = private_file(&previous)?;
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let temporary = format!(
            ".state-{}-{}.tmp",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let mut file = open_at(&self.file, &temporary, true)?;
        let key = private_file(&file)?;
        struct Cleanup<'a> {
            directory: &'a File,
            name: String,
            key: Key,
        }
        impl Drop for Cleanup<'_> {
            fn drop(&mut self) {
                if let Ok(file) = open_at(self.directory, &self.name, false) {
                    if private_file(&file).ok() == Some(self.key) {
                        if let Ok(name) = CString::new(self.name.as_str()) {
                            unsafe {
                                libc::unlinkat(self.directory.as_raw_fd(), name.as_ptr(), 0);
                            }
                        }
                    }
                }
            }
        }
        let _cleanup = Cleanup {
            directory: &self.file,
            name: temporary.clone(),
            key,
        };
        file.write_all(bytes)
            .and_then(|_| file.sync_all())
            .map_err(|_| fail("supervision_store_io"))?;
        self.check()?;
        if private_file(&open_at(&self.file, name, false)?)? != previous_key {
            return Err(fail("supervision_store_changed"));
        }
        let from = CString::new(temporary).map_err(|_| fail("supervision_store_configuration"))?;
        let to = CString::new(name).map_err(|_| fail("supervision_store_configuration"))?;
        if unsafe {
            libc::renameat(
                self.file.as_raw_fd(),
                from.as_ptr(),
                self.file.as_raw_fd(),
                to.as_ptr(),
            )
        } != 0
        {
            return Err(fail("supervision_store_io"));
        }
        self.file
            .sync_all()
            .map_err(|_| fail("supervision_store_io"))
    }
}

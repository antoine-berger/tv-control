//! Best-effort, versioned private label cache. Failed lookups are never cached as labels.
use super::*;
use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    os::unix::fs::PermissionsExt,
};
impl GenericAdb {
    fn cache_file(&self) -> Option<crate::pipeline_store::AtomicFile> {
        let directory = self.cache_dir.as_ref()?;
        if !directory.is_absolute() {
            return None;
        }
        for ancestor in directory.parent()?.ancestors() {
            if ancestor.symlink_metadata().ok()?.file_type().is_symlink() {
                return None;
            }
        }
        match std::fs::create_dir(directory) {
            Ok(()) => {
                std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700)).ok()?
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => return None,
        }
        let m = directory.symlink_metadata().ok()?;
        if !m.is_dir()
            || m.uid() != unsafe { libc::geteuid() }
            || m.permissions().mode() & 0o777 != 0o700
        {
            return None;
        }
        crate::pipeline_store::AtomicFile::open(directory.join("labels.json"), 1024 * 1024).ok()
    }
    pub(super) fn hydrate_labels(
        &self,
        alias: &str,
        candidates: &mut [Candidate],
        query: Option<&str>,
        ctx: &Context<'_>,
    ) -> PortResult<()> {
        if candidates
            .iter()
            .all(|c| c.app.label.is_some() || c.resource.is_none() || c.source.is_none())
        {
            return Ok(());
        }
        // Identity and locale are observations, not user-provided cache selectors.
        let properties = self.shell(alias, &["getprop"], &[], false, ctx)?;
        let prop = |key: &str| -> Option<&str> {
            properties
                .lines()
                .find_map(|line| line.strip_prefix(&format!("[{key}]: ["))?.strip_suffix(']'))
        };
        let locale = prop("persist.sys.locale")
            .filter(|s| !s.is_empty())
            .or_else(|| prop("ro.product.locale"))
            .unwrap_or("");
        let identity = prop("ro.serialno")
            .filter(|s| !s.is_empty())
            .or_else(|| prop("ro.boot.serialno"))
            .unwrap_or("");
        let fingerprint = prop("ro.build.fingerprint").unwrap_or("");
        let target = self.target(alias)?;
        let mut cache = self
            .labels
            .try_lock()
            .map_err(|_| Failure::before("adapter_state_unavailable"))?;
        let mut file = self.cache_file();
        if let Some(saved) = file
            .as_ref()
            .and_then(|f| f.bytes())
            .and_then(|b| serde_json::from_slice::<BTreeMap<String, String>>(b).ok())
        {
            for (key, label) in saved.into_iter().take(4096) {
                if key.len() == 16
                    && key.bytes().all(|b| b.is_ascii_hexdigit())
                    && valid_label(&label)
                {
                    cache.entry(key).or_insert(label);
                }
            }
        }
        let mut changed = false;
        let cursor = self
            .scan_cursor
            .lock()
            .map_err(|_| Failure::before("adapter_state_unavailable"))?
            .get(alias)
            .copied()
            .unwrap_or(0);
        let normalize = |text: &str| {
            text.chars()
                .filter(|c| c.is_alphanumeric())
                .flat_map(char::to_lowercase)
                .collect::<String>()
        };
        let hint = query.map(normalize).filter(|s| !s.is_empty());
        let count = candidates.len();
        let mut order: Vec<_> = (0..count).collect();
        // Package text is only a hydration scheduling hint. It never becomes a
        // display label or authorizes name resolution to an app.
        order.sort_by_key(|i| {
            (
                hint.as_ref()
                    .is_none_or(|q| !normalize(&candidates[*i].app.id).contains(q)),
                (i + count - cursor % count) % count,
            )
        });
        let mut next_cursor = cursor;
        for index in order {
            let candidate = &mut candidates[index];
            if candidate.app.label.is_some() {
                continue;
            }
            let (Some(resource), Some(source)) = (candidate.resource, candidate.source.as_deref())
            else {
                continue;
            };
            let mut h = DefaultHasher::new();
            (
                target,
                identity,
                fingerprint,
                locale,
                &candidate.app.id,
                &candidate.app.component,
                candidate.version,
                source,
                resource,
            )
                .hash(&mut h);
            let key = format!("{:016x}", h.finish());
            // Without version/device identity, no durable reuse: never mistake an update or replacement for the old app.
            let reusable =
                candidate.version.is_some() && !identity.is_empty() && !locale.is_empty();
            if reusable {
                if let Some(label) = cache.get(&key) {
                    candidate.app.label = Some(label.clone());
                    continue;
                }
            }
            if ctx.remaining_ms() <= 750 {
                break;
            }
            next_cursor = (index + 1) % count;
            let sub = Context {
                clock: ctx.clock,
                cancellation: ctx.cancellation,
                deadline_monotonic_ms: ctx
                    .deadline_monotonic_ms
                    .min(ctx.clock.monotonic_ms().saturating_add(350)),
            };
            let table = match self.shell_bytes(
                alias,
                &["unzip", "-p", source, "resources.arsc"],
                &[],
                false,
                crate::apk_labels::MAX_RESOURCE_BYTES,
                &sub,
            ) {
                Ok(table) => table,
                Err(_) => {
                    ctx.check()?;
                    continue;
                }
            };
            let label = crate::apk_labels::resource_label(
                &table,
                resource,
                (!locale.is_empty()).then_some(locale),
            )
            .ok()
            .flatten()
            .filter(|s| valid_label(s));
            if let Some(label) = label {
                candidate.app.label = Some(label.clone());
                if reusable {
                    if cache.len() >= 4096 {
                        cache.clear();
                    }
                    cache.insert(key, label);
                    changed = true;
                }
            }
        }
        ctx.check()?;
        self.scan_cursor
            .lock()
            .map_err(|_| Failure::before("adapter_state_unavailable"))?
            .insert(alias.to_owned(), next_cursor);
        if changed {
            if let Some(file) = &mut file {
                if let Ok(bytes) = serde_json::to_vec(&*cache) {
                    let _ = file.write(bytes);
                }
            }
        }
        Ok(())
    }
}
pub(super) fn valid_label(label: &str) -> bool {
    !label.trim().is_empty() && label.len() <= 256 && !label.chars().any(char::is_control)
}

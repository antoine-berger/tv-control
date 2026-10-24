package org.tvcontrol.nativeinput;

import java.io.ByteArrayOutputStream;
import java.io.DataOutputStream;
import java.io.File;
import java.io.IOException;
import java.nio.ByteBuffer;
import java.nio.channels.FileChannel;
import java.nio.charset.StandardCharsets;
import java.nio.file.DirectoryStream;
import java.nio.file.FileAlreadyExistsException;
import java.nio.file.Files;
import java.nio.file.LinkOption;
import java.nio.file.NoSuchFileException;
import java.nio.file.Path;
import java.nio.file.StandardOpenOption;
import java.nio.file.attribute.PosixFileAttributes;
import java.nio.file.attribute.PosixFilePermissions;
import java.nio.file.attribute.UserPrincipal;
import java.util.Arrays;
import java.util.HashSet;

/** Own scratch only. Construction has no I/O; register before open/cancel. */
final class LabPrivateRequest implements AutoCloseable {
    enum Point { DIRECTORY_CREATED, REQUEST_CREATED, CANCEL_CREATED, REQUEST_CLOSED, CANCEL_CLOSED, DIRECTORY_CLOSED }
    interface Checkpoint { void hit(Point point) throws IOException; }
    private final File contextCache;
    private final IssuedEditor.Entropy entropy;
    private final Checkpoint checkpoint;
    private UserPrincipal cacheOwner;
    private Path base, directory, request, cancel;
    private Object baseKey, directoryKey, requestKey, cancelKey;
    private FileChannel requestPin, cancelPin;
    private DirectoryStream<Path> directoryPin;
    private String token;
    private boolean attempted, opened, closed, directoryCreated, requestCreated, cancelCreated;
    private boolean allocationUnknown, verified, failed;

    LabPrivateRequest(File trustedContextCache) { this(trustedContextCache, null, null); }
    LabPrivateRequest(File trustedContextCache, IssuedEditor.Entropy random) {
        this(trustedContextCache, random, null);
    }
    // Package-private host-test seam only; never selected by ENV/argv/property.
    LabPrivateRequest(File trustedContextCache, IssuedEditor.Entropy random, Checkpoint observer) {
        contextCache = trustedContextCache; entropy = random; checkpoint = observer;
    }
    private void hit(Point point) throws IOException { if (checkpoint != null) checkpoint.hit(point); }
    private static LabProtocol.Fault fault(LabProtocol.Reason reason) { return new LabProtocol.Fault(reason); }
    private static PosixFileAttributes attributes(Path path) throws IOException {
        return Files.readAttributes(path, PosixFileAttributes.class, LinkOption.NOFOLLOW_LINKS);
    }
    private static Object identity(PosixFileAttributes value) throws IOException {
        Object key = value.fileKey();
        if (key == null) throw new IOException();
        return key;
    }
    private static boolean same(Path path, Object key, boolean dir) throws IOException {
        PosixFileAttributes value = attributes(path);
        return key != null && key.equals(value.fileKey()) && !value.isSymbolicLink()
            && (dir ? value.isDirectory() : value.isRegularFile());
    }
    private String newToken() {
        byte[] bytes = new byte[16];
        (entropy == null ? IssuedEditor.secureEntropy() : entropy).fill(bytes);
        char[] hex = new char[32];
        final char[] alphabet = "0123456789abcdef".toCharArray();
        for (int i = 0; i < bytes.length; i++) {
            hex[2*i] = alphabet[(bytes[i] & 255) >>> 4]; hex[2*i+1] = alphabet[bytes[i] & 15];
        }
        Arrays.fill(bytes, (byte)0);
        return new String(hex);
    }
    private static void text(DataOutputStream out, String value) throws IOException {
        byte[] encoded = value.getBytes(StandardCharsets.UTF_8);
        if (encoded.length > 2048 || !value.equals(new String(encoded, StandardCharsets.UTF_8))) throw new IOException();
        out.writeInt(encoded.length); out.write(encoded);
    }
    private static byte[] encode(IssuedEditor.PendingText pending) throws IOException {
        if (pending == null || pending.expires <= 0) throw new IOException();
        EditorBinding binding = pending.binding;
        ByteArrayOutputStream bytes = new ByteArrayOutputStream();
        DataOutputStream out = new DataOutputStream(bytes);
        out.writeInt(0x54564e31); out.writeLong(pending.expires);
        text(out, binding.packageName); text(out, binding.resource); text(out, binding.role);
        out.writeInt(binding.window); out.writeInt(binding.left); out.writeInt(binding.top);
        out.writeInt(binding.right); out.writeInt(binding.bottom);
        text(out, binding.before()); text(out, pending.after()); out.flush();
        if (bytes.size() > 16384) throw new IOException();
        return bytes.toByteArray();
    }
    private static FileChannel createFile(Path path) throws IOException {
        return FileChannel.open(path, new HashSet<java.nio.file.OpenOption>(Arrays.asList(
            StandardOpenOption.CREATE_NEW, StandardOpenOption.WRITE, LinkOption.NOFOLLOW_LINKS)),
            PosixFilePermissions.asFileAttribute(PosixFilePermissions.fromString("rw-------")));
    }
    synchronized void open(IssuedEditor.PendingText pending) throws LabProtocol.Fault {
        if (attempted || closed) throw fault(LabProtocol.Reason.REF_CONSUMED);
        attempted = true;
        byte[] payload = null;
        try {
            payload = encode(pending);
            if (contextCache == null) throw new IOException();
            Path supplied = contextCache.toPath();
            if (!supplied.isAbsolute() || !supplied.equals(supplied.normalize()) || Files.isSymbolicLink(supplied))
                throw new IOException();
            // Context is the trust source. Canonicalize legitimate system ancestors.
            base = supplied.toRealPath();
            PosixFileAttributes cache = attributes(base);
            if (!cache.isDirectory() || cache.isSymbolicLink()) throw new IOException();
            baseKey = identity(cache);
            cacheOwner = cache.owner();
            token = newToken();
            directory = base.resolve("tv-native-text-" + token);
            if (!directory.getParent().equals(base)) throw new IOException();
            request = directory.resolve("request"); cancel = directory.resolve("cancel");
            allocationUnknown = true;
            try {
                Files.createDirectory(directory, PosixFilePermissions.asFileAttribute(PosixFilePermissions.fromString("rwx------")));
            } catch (FileAlreadyExistsException collision) {
                allocationUnknown = false; throw collision; // Existing path is never ours.
            }
            directoryCreated = true;
            hit(Point.DIRECTORY_CREATED);
            PosixFileAttributes dir = attributes(directory);
            directoryKey = identity(dir);
            if (!dir.isDirectory() || dir.isSymbolicLink() || !dir.owner().equals(cache.owner())
                    || !dir.permissions().equals(PosixFilePermissions.fromString("rwx------"))) throw new IOException();
            directoryPin = Files.newDirectoryStream(directory);
            requestPin = createFile(request); requestCreated = true;
            hit(Point.REQUEST_CREATED);
            PosixFileAttributes file = attributes(request); requestKey = identity(file);
            if (!file.isRegularFile() || file.isSymbolicLink() || !file.owner().equals(dir.owner())
                    || !file.permissions().equals(PosixFilePermissions.fromString("rw-------"))) throw new IOException();
            allocationUnknown = false;
            ByteBuffer buffer = ByteBuffer.wrap(payload);
            while (buffer.hasRemaining()) if (requestPin.write(buffer) <= 0) throw new IOException();
            if (!same(base, baseKey, true) || !same(directory, directoryKey, true) || !same(request, requestKey, false))
                throw new IOException();
            // Keep the inode pinned until cleanup, including after engine unlink.
            opened = true;
        } catch (Throwable failure) {
            failed = true;
            throw fault(LabProtocol.Reason.TRANSPORT_LOST);
        } finally { if (payload != null) Arrays.fill(payload, (byte)0); }
    }
    synchronized String token() throws LabProtocol.Fault {
        if (!opened || closed || failed) throw fault(LabProtocol.Reason.REF_CONSUMED);
        return token;
    }
    synchronized void cancel() throws LabProtocol.Fault {
        if (!opened || closed || failed) throw fault(LabProtocol.Reason.REF_CONSUMED);
        try {
            if (!same(base, baseKey, true) || !same(directory, directoryKey, true)) throw new IOException();
            if (cancelCreated) {
                if (!same(cancel, cancelKey, false)) throw new IOException();
                return;
            }
            allocationUnknown = true;
            cancelPin = createFile(cancel); cancelCreated = true;
            hit(Point.CANCEL_CREATED);
            PosixFileAttributes value = attributes(cancel); cancelKey = identity(value);
            if (!value.isRegularFile() || value.isSymbolicLink() || !value.owner().equals(cacheOwner)
                    || !value.permissions().equals(PosixFilePermissions.fromString("rw-------"))) throw new IOException();
            allocationUnknown = false;
        } catch (Throwable failure) { failed = true; throw fault(LabProtocol.Reason.TRANSPORT_LOST); }
    }
    private static boolean removeOwned(Path path, Object key, boolean created) {
        try {
            PosixFileAttributes value = attributes(path);
            if (!created || value.isSymbolicLink() || !value.isRegularFile()
                    || key == null || !key.equals(value.fileKey())) return false;
            Files.delete(path); return true;
        } catch (NoSuchFileException absent) { return true; }
        catch (Throwable failure) { return false; }
    }
    @Override public synchronized void close() throws LabProtocol.Fault {
        if (closed) {
            if (!verified) throw fault(LabProtocol.Reason.CLEANUP_UNVERIFIED);
            return;
        }
        closed = true;
        boolean clean = !allocationUnknown;
        try {
            if (directoryCreated && directoryKey != null && same(base, baseKey, true)
                    && same(directory, directoryKey, true)) {
                boolean removedRequest = removeOwned(request, requestKey, requestCreated);
                boolean removedCancel = removeOwned(cancel, cancelKey, cancelCreated);
                clean &= removedRequest && removedCancel;
                if (clean && same(directory, directoryKey, true)) Files.delete(directory);
            } else if (directoryCreated || allocationUnknown) clean = false;
        } catch (Throwable failure) { clean = false; }
        try { if (requestPin != null) { requestPin.close(); hit(Point.REQUEST_CLOSED); } } catch (Throwable failure) { clean = false; }
        try { if (cancelPin != null) { cancelPin.close(); hit(Point.CANCEL_CLOSED); } } catch (Throwable failure) { clean = false; }
        try { if (directoryPin != null) { directoryPin.close(); hit(Point.DIRECTORY_CLOSED); } } catch (Throwable failure) { clean = false; }
        requestPin = null; cancelPin = null; directoryPin = null; token = null;
        verified = clean;
        if (!verified) throw fault(LabProtocol.Reason.CLEANUP_UNVERIFIED);
    }
    synchronized boolean cleanupVerified() { return closed && verified; }
    @Override public String toString() { return "LabPrivateRequest"; }
}

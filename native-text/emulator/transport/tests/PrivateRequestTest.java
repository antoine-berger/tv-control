package org.tvcontrol.nativeinput;

import java.io.ByteArrayInputStream;
import java.io.ByteArrayOutputStream;
import java.io.DataInputStream;
import java.io.DataOutputStream;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.LinkOption;
import java.nio.file.Path;
import java.nio.file.attribute.PosixFilePermissions;
import java.util.Arrays;

/** Actual scratch owner and actual engine decoder; host filesystem only. */
public final class PrivateRequestTest {
    static int checks;
    interface Checked { void run() throws Exception; }
    static void check(boolean value) { checks++; if (!value) throw new AssertionError("private request invariant"); }
    static void fault(LabProtocol.Reason reason, Checked call) throws Exception {
        try { call.run(); } catch (LabProtocol.Fault failure) {
            check(failure.reason == reason); check(failure.getMessage().equals(reason.name()));
            check(failure.getCause() == null); return;
        }
        throw new AssertionError("expected fixed private-request refusal");
    }
    static IssuedEditor.PendingText pending() throws Exception {
        IssuedEditor operation = new IssuedEditor(() -> 100, bytes -> Arrays.fill(bytes, (byte)1), 2100);
        EditorBinding binding = new EditorBinding("fixture.pkg", "fixture.pkg:id/editor", "android.widget.EditText",
            17, 1, 2, 100, 40, "seed", 4, 4);
        IssuedEditor.Lease ref = operation.issue(binding, 100);
        byte[] append = " é🙂%s'".getBytes(StandardCharsets.UTF_8);
        ByteArrayOutputStream bytes = new ByteArrayOutputStream(); DataOutputStream out = new DataOutputStream(bytes);
        out.writeInt(LabProtocol.MAGIC); out.writeByte(1); out.writeByte(2);
        out.write(ref.epoch()); out.write(ref.reference()); out.writeInt(append.length); out.write(append);
        byte[] body = bytes.toByteArray(); bytes.reset(); out = new DataOutputStream(bytes);
        out.writeInt(body.length); out.write(body);
        return operation.consume(LabProtocol.decode(bytes.toByteArray(), new LabProtocol.InputBudget()));
    }
    static LabPrivateRequest owner(Path cache, int value) {
        return new LabPrivateRequest(cache.toFile(), bytes -> Arrays.fill(bytes, (byte)value));
    }
    static String fixedToken(int value) {
        StringBuilder result = new StringBuilder();
        for (int i = 0; i < 16; i++) result.append(String.format(java.util.Locale.ROOT, "%02x", value));
        return result.toString();
    }
    static Path directory(Path cache, String token) { return cache.resolve("tv-native-text-" + token); }
    static void positive(Path root) throws Exception {
        Path cache = Files.createDirectory(root.resolve("positive"));
        final int[] fills = {0}; final byte[][] retained = {null};
        LabPrivateRequest request = new LabPrivateRequest(cache.toFile(), bytes -> {
            fills[0]++; Arrays.fill(bytes, (byte)0x21); retained[0] = bytes;
        });
        check(fills[0] == 0 && !request.cleanupVerified());
        IssuedEditor.PendingText text = pending(); request.open(text);
        String token = request.token(); Arrays.fill(retained[0], (byte)0x7f);
        check(token.matches("[0-9a-f]{32}") && token.equals(request.token()));
        check(!request.toString().contains(token) && !request.toString().contains(text.after()));
        Path dir = directory(cache, token), file = dir.resolve("request");
        check(Files.getPosixFilePermissions(dir).equals(PosixFilePermissions.fromString("rwx------")));
        check(Files.getPosixFilePermissions(file).equals(PosixFilePermissions.fromString("rw-------")));
        byte[] encoded = Files.readAllBytes(file); check(encoded.length <= 16384);
        RequestDecoderProbe.Request decoded = new RequestDecoderProbe.Request(new DataInputStream(new ByteArrayInputStream(encoded)));
        check(decoded.expires == 2100 && decoded.expires == text.expires);
        check(decoded.pkg.equals(text.binding.packageName) && decoded.resource.equals(text.binding.resource));
        check(decoded.role.equals(text.binding.role) && decoded.window == text.binding.window);
        check(decoded.bounds.left == 1 && decoded.bounds.top == 2 && decoded.bounds.right == 100 && decoded.bounds.bottom == 40);
        check(decoded.before.equals("seed") && decoded.after.equals("seed é🙂%s'"));
        fault(LabProtocol.Reason.REF_CONSUMED, () -> request.open(text));
        check(Arrays.equals(encoded, Files.readAllBytes(file)));
        request.cancel(); Path marker = dir.resolve("cancel"); Object markerKey = Files.readAttributes(marker,
            java.nio.file.attribute.BasicFileAttributes.class, LinkOption.NOFOLLOW_LINKS).fileKey();
        request.cancel(); check(Files.size(marker) == 0);
        check(markerKey.equals(Files.readAttributes(marker, java.nio.file.attribute.BasicFileAttributes.class).fileKey()));
        check(Files.getPosixFilePermissions(marker).equals(PosixFilePermissions.fromString("rw-------")));
        Files.delete(file); // The actual engine consumes its request before mutation.
        request.close(); check(request.cleanupVerified() && !Files.exists(dir, LinkOption.NOFOLLOW_LINKS));
        Files.createDirectory(dir); Files.write(dir.resolve("foreign"), new byte[]{1});
        request.close(); check(Files.exists(dir.resolve("foreign"))); // No second destruction.
        fault(LabProtocol.Reason.REF_CONSUMED, () -> request.token());
        fault(LabProtocol.Reason.REF_CONSUMED, () -> request.cancel());
    }
    static void collisionsAndLinks(Path root) throws Exception {
        Path cache = Files.createDirectory(root.resolve("collisions"));
        String token = fixedToken(0x33); Path collision = directory(cache, token);
        Files.createDirectory(collision); Path sentinel = Files.write(collision.resolve("foreign"), new byte[]{8});
        LabPrivateRequest existing = owner(cache, 0x33);
        fault(LabProtocol.Reason.TRANSPORT_LOST, () -> existing.open(pending()));
        existing.close(); check(existing.cleanupVerified() && Files.readAllBytes(sentinel)[0] == 8);
        Path outside = Files.createDirectory(root.resolve("outside")); Files.write(outside.resolve("keep"), new byte[]{9});
        Path link = directory(cache, fixedToken(0x44)); Files.createSymbolicLink(link, outside);
        LabPrivateRequest linked = owner(cache, 0x44);
        fault(LabProtocol.Reason.TRANSPORT_LOST, () -> linked.open(pending()));
        linked.close(); check(Files.isSymbolicLink(link) && Files.readAllBytes(outside.resolve("keep"))[0] == 9);
        Path baseLink = root.resolve("base-link"); Files.createSymbolicLink(baseLink, cache);
        LabPrivateRequest linkBase = owner(baseLink, 0x55);
        fault(LabProtocol.Reason.TRANSPORT_LOST, () -> linkBase.open(pending())); linkBase.close();
        check(linkBase.cleanupVerified() && !Files.exists(directory(cache, fixedToken(0x55))));
        Path realParent = Files.createDirectory(root.resolve("real-parent"));
        Path realCache = Files.createDirectory(realParent.resolve("cache"));
        Path ancestor = root.resolve("system-ancestor"); Files.createSymbolicLink(ancestor, realParent);
        LabPrivateRequest allowedAncestor = owner(ancestor.resolve("cache"), 0x66);
        allowedAncestor.open(pending()); allowedAncestor.close(); check(allowedAncestor.cleanupVerified());
        check(!Files.exists(directory(realCache, fixedToken(0x66))));
        for (Path unsafe : new Path[]{java.nio.file.Paths.get("relative-cache"), cache.resolve("../outside")}) {
            LabPrivateRequest rejected = owner(unsafe, 0x77);
            fault(LabProtocol.Reason.TRANSPORT_LOST, () -> rejected.open(pending())); rejected.close();
            check(rejected.cleanupVerified());
        }
    }
    static void replacementsAndUnknownEntries(Path root) throws Exception {
        Path cache = Files.createDirectory(root.resolve("replacements"));
        for (boolean link : new boolean[]{false, true}) {
            LabPrivateRequest owner = owner(cache, link ? 0x81 : 0x80); owner.open(pending());
            Path dir = directory(cache, owner.token()), file = dir.resolve("request");
            Object original = Files.readAttributes(file, java.nio.file.attribute.BasicFileAttributes.class).fileKey();
            Files.delete(file);
            Path outside = Files.write(root.resolve(link ? "link-target" : "replacement-target"), new byte[]{12});
            if (link) Files.createSymbolicLink(file, outside); else Files.write(file, new byte[]{13});
            check(link || !original.equals(Files.readAttributes(file, java.nio.file.attribute.BasicFileAttributes.class).fileKey()));
            fault(LabProtocol.Reason.CLEANUP_UNVERIFIED, () -> owner.close());
            check(!owner.cleanupVerified() && Files.exists(file, LinkOption.NOFOLLOW_LINKS));
            check(Files.readAllBytes(outside)[0] == 12);
            if (!link) check(Files.readAllBytes(file)[0] == 13);
            fault(LabProtocol.Reason.CLEANUP_UNVERIFIED, () -> owner.close());
            check(Files.exists(file, LinkOption.NOFOLLOW_LINKS));
        }
        LabPrivateRequest unexpected = owner(cache, 0x82); unexpected.open(pending());
        Path dir = directory(cache, unexpected.token()), extra = Files.write(dir.resolve("foreign"), new byte[]{14});
        fault(LabProtocol.Reason.CLEANUP_UNVERIFIED, () -> unexpected.close());
        check(Files.readAllBytes(extra)[0] == 14 && !unexpected.cleanupVerified());
        Files.delete(extra);
        fault(LabProtocol.Reason.CLEANUP_UNVERIFIED, () -> unexpected.close()); check(Files.exists(dir));
        LabPrivateRequest markerCollision = owner(cache, 0x83); markerCollision.open(pending());
        Path markerDir = directory(cache, markerCollision.token());
        Path foreignMarker = Files.write(markerDir.resolve("cancel"), new byte[]{15});
        fault(LabProtocol.Reason.TRANSPORT_LOST, () -> markerCollision.cancel());
        fault(LabProtocol.Reason.REF_CONSUMED, () -> markerCollision.cancel());
        fault(LabProtocol.Reason.CLEANUP_UNVERIFIED, () -> markerCollision.close());
        check(Files.readAllBytes(foreignMarker)[0] == 15);
        LabPrivateRequest moved = owner(cache, 0x84); moved.open(pending());
        Path oldDir = directory(cache, moved.token()), newDir = cache.resolve("moved-owned");
        Files.move(oldDir, newDir); Files.createDirectory(oldDir); Files.write(oldDir.resolve("foreign"), new byte[]{16});
        fault(LabProtocol.Reason.CLEANUP_UNVERIFIED, () -> moved.close());
        check(Files.exists(newDir.resolve("request")) && Files.readAllBytes(oldDir.resolve("foreign"))[0] == 16);
    }
    static void partialFailure(Path root) throws Exception {
        Path cache = Files.createDirectory(root.resolve("partial"));
        LabPrivateRequest unopened = owner(cache, 0x90); unopened.close(); check(unopened.cleanupVerified());
        LabPrivateRequest noRequest = owner(cache, 0x91);
        fault(LabProtocol.Reason.TRANSPORT_LOST, () -> noRequest.open(null)); noRequest.close(); check(noRequest.cleanupVerified());
        LabPrivateRequest entropy = new LabPrivateRequest(cache.toFile(), bytes -> { throw new AssertionError("synthetic-private-text"); });
        fault(LabProtocol.Reason.TRANSPORT_LOST, () -> entropy.open(pending()));
        entropy.close(); check(entropy.cleanupVerified());
        fault(LabProtocol.Reason.REF_CONSUMED, () -> entropy.token());
    }
    static void allocationAndCloseFailures(Path root) throws Exception {
        for (LabPrivateRequest.Point failedPoint : LabPrivateRequest.Point.values()) {
            Path cache = Files.createDirectory(root.resolve("injected-" + failedPoint.name()));
            java.util.EnumMap<LabPrivateRequest.Point,Integer> hits = new java.util.EnumMap<>(LabPrivateRequest.Point.class);
            LabPrivateRequest owner = new LabPrivateRequest(cache.toFile(), bytes -> Arrays.fill(bytes, (byte)0xa0), point -> {
                hits.put(point, hits.getOrDefault(point, 0) + 1);
                if (point == failedPoint) throw new java.io.IOException("synthetic-private-text");
            });
            boolean creationFailure = failedPoint == LabPrivateRequest.Point.DIRECTORY_CREATED
                || failedPoint == LabPrivateRequest.Point.REQUEST_CREATED || failedPoint == LabPrivateRequest.Point.CANCEL_CREATED;
            Path dir = directory(cache, fixedToken(0xa0));
            if (failedPoint == LabPrivateRequest.Point.DIRECTORY_CREATED || failedPoint == LabPrivateRequest.Point.REQUEST_CREATED)
                fault(LabProtocol.Reason.TRANSPORT_LOST, () -> owner.open(pending()));
            else {
                owner.open(pending());
                if (failedPoint == LabPrivateRequest.Point.CANCEL_CREATED)
                    fault(LabProtocol.Reason.TRANSPORT_LOST, () -> owner.cancel());
                else owner.cancel();
            }
            fault(LabProtocol.Reason.CLEANUP_UNVERIFIED, () -> owner.close());
            check(!owner.cleanupVerified() && hits.get(failedPoint) == 1);
            if (creationFailure) {
                check(Files.exists(dir));
                if (failedPoint == LabPrivateRequest.Point.REQUEST_CREATED) check(Files.exists(dir.resolve("request")));
                if (failedPoint == LabPrivateRequest.Point.CANCEL_CREATED) check(Files.exists(dir.resolve("cancel")));
            } else {
                check(!Files.exists(dir));
                check(hits.get(LabPrivateRequest.Point.REQUEST_CLOSED) == 1);
                check(hits.get(LabPrivateRequest.Point.CANCEL_CLOSED) == 1);
                check(hits.get(LabPrivateRequest.Point.DIRECTORY_CLOSED) == 1);
            }
            java.util.EnumMap<LabPrivateRequest.Point,Integer> once = new java.util.EnumMap<>(hits);
            fault(LabProtocol.Reason.CLEANUP_UNVERIFIED, () -> owner.close());
            check(hits.equals(once)); // No second close/delete/reconciliation after uncertainty.
            if (creationFailure) check(Files.exists(dir));
        }
    }
    /** These are limitation witnesses, NOT a claim that same-UID replacement is resisted. */
    static void sameUidPreBaselineWindowsOutsideModel(Path root) throws Exception {
        LabPrivateRequest.Point[] windows={LabPrivateRequest.Point.DIRECTORY_CREATED,
            LabPrivateRequest.Point.REQUEST_CREATED,LabPrivateRequest.Point.CANCEL_CREATED};
        for(int index=0;index<windows.length;index++) {
            LabPrivateRequest.Point window=windows[index];
            Path cache=Files.createDirectory(root.resolve("outside-model-"+window));
            int entropy=0xb0+index;
            Path dir=directory(cache,fixedToken(entropy));
            Path original=root.resolve("moved-original-"+window),witness=root.resolve("foreign-hardlink-"+window);
            final Object[] keys=new Object[2];
            java.util.EnumMap<LabPrivateRequest.Point,Integer> hits=new java.util.EnumMap<>(LabPrivateRequest.Point.class);
            LabPrivateRequest owner=new LabPrivateRequest(cache.toFile(),bytes->Arrays.fill(bytes,(byte)entropy),point->{
                hits.put(point,hits.getOrDefault(point,0)+1);
                if(point!=window)return;
                Path path=window==LabPrivateRequest.Point.DIRECTORY_CREATED ? dir
                    : dir.resolve(window==LabPrivateRequest.Point.REQUEST_CREATED?"request":"cancel");
                keys[0]=Files.readAttributes(path,java.nio.file.attribute.BasicFileAttributes.class,LinkOption.NOFOLLOW_LINKS).fileKey();
                Files.move(path,original); // Preserve the first-created object, preventing inode reuse.
                if(window==LabPrivateRequest.Point.DIRECTORY_CREATED)
                    Files.createDirectory(path,PosixFilePermissions.asFileAttribute(PosixFilePermissions.fromString("rwx------")));
                else {
                    Files.createFile(path,PosixFilePermissions.asFileAttribute(PosixFilePermissions.fromString("rw-------")));
                    Files.write(path,new byte[]{42,43});
                    Files.createLink(witness,path); // Preserve independent evidence of the adopted foreign inode.
                }
                keys[1]=Files.readAttributes(path,java.nio.file.attribute.BasicFileAttributes.class,LinkOption.NOFOLLOW_LINKS).fileKey();
                check(keys[0]!=null && keys[1]!=null && !keys[0].equals(keys[1]));
            });
            owner.open(pending());
            if(window==LabPrivateRequest.Point.CANCEL_CREATED)owner.cancel();
            if(window==LabPrivateRequest.Point.DIRECTORY_CREATED) {
                Files.createLink(witness,dir.resolve("request"));
                try(java.nio.file.DirectoryStream<Path> empty=Files.newDirectoryStream(original)){check(!empty.iterator().hasNext());}
            } else {
                Path path=dir.resolve(window==LabPrivateRequest.Point.REQUEST_CREATED?"request":"cancel");
                check(Files.isSameFile(path,witness));check(Arrays.equals(Files.readAllBytes(witness),new byte[]{42,43}));
            }
            if(window==LabPrivateRequest.Point.REQUEST_CREATED) {
                // The retained channel wrote the payload to the moved original,
                // while the pathname baseline adopted a different foreign inode.
                RequestDecoderProbe.Request decoded=new RequestDecoderProbe.Request(new DataInputStream(
                    new ByteArrayInputStream(Files.readAllBytes(original))));
                check(decoded.before.equals("seed") && decoded.after.equals(pending().after()));
            }
            if(window==LabPrivateRequest.Point.CANCEL_CREATED)check(Files.size(original)==0);
            owner.close();
            // This true result is conditional on an assumption deliberately broken
            // here. It does not detect/prove exclusivity or authenticate creation.
            check(owner.cleanupVerified());check(!Files.exists(dir,LinkOption.NOFOLLOW_LINKS));
            check(Files.exists(original,LinkOption.NOFOLLOW_LINKS));
            check(keys[0].equals(Files.readAttributes(original,java.nio.file.attribute.BasicFileAttributes.class).fileKey()));
            check(Files.exists(witness));
            if(window!=LabPrivateRequest.Point.DIRECTORY_CREATED) {
                check(keys[1].equals(Files.readAttributes(witness,java.nio.file.attribute.BasicFileAttributes.class).fileKey()));
                check(Arrays.equals(Files.readAllBytes(witness),new byte[]{42,43}));
            }
            check(hits.get(window)==1 && hits.get(LabPrivateRequest.Point.REQUEST_CLOSED)==1
                && hits.get(LabPrivateRequest.Point.DIRECTORY_CLOSED)==1);
            if(window==LabPrivateRequest.Point.CANCEL_CREATED)check(hits.get(LabPrivateRequest.Point.CANCEL_CLOSED)==1);
        }
    }
    public static void main(String[] args) throws Exception {
        Path root = Files.createTempDirectory("private-request-host-").toRealPath();
        try {
            positive(root); collisionsAndLinks(root); replacementsAndUnknownEntries(root); partialFailure(root);
            allocationAndCloseFailures(root);
            sameUidPreBaselineWindowsOutsideModel(root);
            System.out.println("PASS private request host checks: " + checks);
        } finally {
            Files.walkFileTree(root, new java.nio.file.SimpleFileVisitor<Path>() {
                @Override public java.nio.file.FileVisitResult visitFile(Path path, java.nio.file.attribute.BasicFileAttributes attrs) throws java.io.IOException {
                    Files.delete(path); return java.nio.file.FileVisitResult.CONTINUE;
                }
                @Override public java.nio.file.FileVisitResult postVisitDirectory(Path path, java.io.IOException error) throws java.io.IOException {
                    Files.delete(path); return java.nio.file.FileVisitResult.CONTINUE;
                }
            });
        }
    }
}

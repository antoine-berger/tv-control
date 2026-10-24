package org.tvcontrol.nativeinput;

import java.nio.charset.StandardCharsets;

/** Immutable observed metadata only; native safety/focus admission is still required. */
final class EditorBinding {
    final String packageName, resource, role;
    final int window, left, top, right, bottom, selectionStart, selectionEnd;
    private final String before;

    EditorBinding(String pkg, String id, String cls, int windowId,
            int x1, int y1, int x2, int y2, String text, int start, int end)
            throws LabProtocol.Fault {
        if (!field(pkg) || !field(id) || !field(cls) || text == null
                || text.length() > 256 || !utf8(text) || windowId < 0 || x2 <= x1 || y2 <= y1
                || start != text.length() || end != text.length())
            throw new LabProtocol.Fault(LabProtocol.Reason.EDITOR_UNSUPPORTED);
        packageName = pkg; resource = id; role = cls; window = windowId;
        left = x1; top = y1; right = x2; bottom = y2;
        before = text; selectionStart = start; selectionEnd = end;
    }
    private static boolean field(String value) {
        if (value == null || value.isEmpty() || value.length() > 2048 || !utf8(value)) return false;
        for (int i = 0; i < value.length(); i++) if (Character.isISOControl(value.charAt(i))) return false;
        return true;
    }
    private static boolean utf8(String value) {
        byte[] encoded = value.getBytes(StandardCharsets.UTF_8);
        return encoded.length <= 2048 && value.equals(new String(encoded, StandardCharsets.UTF_8));
    }
    String before() { return before; }
    String append(String addition) throws LabProtocol.Fault {
        if (addition == null || addition.isEmpty() || !utf8(addition)
                || addition.length() > 256 - before.length())
            throw new LabProtocol.Fault(LabProtocol.Reason.LIMIT_EXCEEDED);
        return before + addition;
    }
    @Override public String toString() { return "ObservedEditorBinding"; }
}

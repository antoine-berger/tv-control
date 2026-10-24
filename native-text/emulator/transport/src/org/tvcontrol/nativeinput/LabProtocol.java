package org.tvcontrol.nativeinput;

import java.io.ByteArrayInputStream;
import java.io.DataInputStream;
import java.nio.charset.StandardCharsets;
import java.util.Arrays;

/** Lab-only fixed wire contract. No Android, I/O dispatch or arbitrary error text. */
final class LabProtocol {
    static final int MAGIC = 0x4e544c31;
    static final int MAX_FRAME_BYTES = 16384;
    static final int MAX_INPUT_BYTES = 32768;
    static final int MAX_INPUT_FRAMES = 6;
    static final int NONCE_BYTES = 16;
    enum Status { REFUSED, UNCERTAIN, VERIFIED }
    enum Reason { NONE, PEER_UNKNOWN, PEER_MISMATCH, POLICY_DENIED, IDENTITY_CHANGED,
        INVALID_FRAME, LIMIT_EXCEEDED, EDITOR_UNSUPPORTED, REF_UNKNOWN, REF_CONSUMED,
        REF_EXPIRED, CANCELLED, DEADLINE, ENGINE_REFUSED, ENGINE_UNCERTAIN,
        TRANSPORT_LOST, CLEANUP_UNVERIFIED }
    enum Opcode { OBSERVE, COMMIT, CANCEL }

    static final class Fault extends Exception {
        final Reason reason;
        Fault(Reason value) { super(value.name()); reason = value; }
    }
    static final class InputBudget {
        private int bytes;
        private int frames;
        synchronized void charge(int length) throws Fault {
            if (length < 0 || length > MAX_FRAME_BYTES
                    || length > MAX_INPUT_BYTES - bytes || frames == MAX_INPUT_FRAMES)
                throw new Fault(Reason.LIMIT_EXCEEDED);
            bytes += length;
            frames++;
        }
    }
    static final class Command {
        final Opcode opcode;
        private final byte[] epoch;
        private final byte[] reference;
        private final String append;
        private Command(Opcode operation, byte[] run, byte[] ref, String text) {
            opcode = operation;
            epoch = run == null ? null : run.clone();
            reference = ref == null ? null : ref.clone();
            append = text;
        }
        byte[] epoch() { return epoch == null ? null : epoch.clone(); }
        byte[] reference() { return reference == null ? null : reference.clone(); }
        String append() { return append; }
        @Override public String toString() { return "LabCommand"; }
    }

    // Validate this prefix before any body allocation in the eventual I/O layer.
    static int checkedBodyLength(int length) throws Fault {
        if (length < 6) throw new Fault(Reason.INVALID_FRAME);
        if (length > MAX_FRAME_BYTES - 4) throw new Fault(Reason.LIMIT_EXCEEDED);
        return length;
    }
    static Command decode(byte[] frame, InputBudget budget) throws Fault {
        if (frame == null || budget == null) throw new Fault(Reason.INVALID_FRAME);
        budget.charge(frame.length);
        try {
            DataInputStream input = new DataInputStream(new ByteArrayInputStream(frame.clone()));
            int bodyLength = checkedBodyLength(input.readInt());
            if (frame.length != bodyLength + 4 || input.readInt() != MAGIC
                    || input.readUnsignedByte() != 1) throw new Fault(Reason.INVALID_FRAME);
            int operation = input.readUnsignedByte();
            Command command;
            if (operation == 1) command = new Command(Opcode.OBSERVE, null, null, null);
            else if (operation == 3) command = new Command(Opcode.CANCEL, null, null, null);
            else if (operation == 2) {
                byte[] epoch = new byte[NONCE_BYTES], reference = new byte[NONCE_BYTES];
                input.readFully(epoch); input.readFully(reference);
                int length = input.readInt();
                if (length <= 0 || length > 2048) throw new Fault(Reason.LIMIT_EXCEEDED);
                byte[] encoded = new byte[length]; input.readFully(encoded);
                String append = new String(encoded, StandardCharsets.UTF_8);
                if (!Arrays.equals(encoded, append.getBytes(StandardCharsets.UTF_8)))
                    throw new Fault(Reason.INVALID_FRAME);
                if (append.length() > 256) throw new Fault(Reason.LIMIT_EXCEEDED);
                command = new Command(Opcode.COMMIT, epoch, reference, append);
            } else throw new Fault(Reason.INVALID_FRAME);
            if (input.read() != -1) throw new Fault(Reason.INVALID_FRAME);
            return command;
        } catch (Fault failure) {
            throw failure;
        } catch (java.io.IOException failure) {
            throw new Fault(Reason.INVALID_FRAME);
        }
    }
    private LabProtocol() {}
}

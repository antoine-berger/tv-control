package org.tvcontrol.nativeinput;

import java.io.ByteArrayInputStream;
import java.io.ByteArrayOutputStream;
import java.io.DataInputStream;
import java.io.DataOutputStream;

/** Closed private response frames. No text or arbitrary diagnostics. */
final class LabReplies {
    static final class Reference {
        private final byte[] epoch, reference;
        final long expires;
        private Reference(byte[] run, byte[] ref, long deadline) {
            epoch=run.clone(); reference=ref.clone(); expires=deadline;
        }
        byte[] epoch(){return epoch.clone();}
        byte[] reference(){return reference.clone();}
        @Override public String toString(){return "PrivateReferenceReply";}
    }
    private static byte[] wrap(int opcode, byte[] fields) throws LabProtocol.Fault {
        try {
            ByteArrayOutputStream bytes=new ByteArrayOutputStream();
            DataOutputStream output=new DataOutputStream(bytes);
            output.writeInt(fields.length+6); output.writeInt(LabProtocol.MAGIC);
            output.writeByte(1); output.writeByte(opcode); output.write(fields); output.flush();
            return bytes.toByteArray();
        } catch(java.io.IOException error){throw new LabProtocol.Fault(LabProtocol.Reason.INVALID_FRAME);}
    }
    static byte[] reference(IssuedEditor.Lease lease) throws LabProtocol.Fault {
        if(lease==null || lease.expires<=0)throw new LabProtocol.Fault(LabProtocol.Reason.INVALID_FRAME);
        try {
            ByteArrayOutputStream bytes=new ByteArrayOutputStream();
            DataOutputStream output=new DataOutputStream(bytes);
            output.write(lease.epoch()); output.write(lease.reference()); output.writeLong(lease.expires);
            return wrap(0x81,bytes.toByteArray());
        } catch(java.io.IOException error){throw new LabProtocol.Fault(LabProtocol.Reason.INVALID_FRAME);}
    }
    static Reference parseReference(byte[] frame) throws LabProtocol.Fault {
        if(frame==null || frame.length!=50)throw new LabProtocol.Fault(LabProtocol.Reason.INVALID_FRAME);
        try {
            DataInputStream input=new DataInputStream(new ByteArrayInputStream(frame.clone()));
            if(input.readInt()!=46 || input.readInt()!=LabProtocol.MAGIC
                    || input.readUnsignedByte()!=1 || input.readUnsignedByte()!=0x81)
                throw new LabProtocol.Fault(LabProtocol.Reason.INVALID_FRAME);
            byte[] epoch=new byte[16],reference=new byte[16];
            input.readFully(epoch); input.readFully(reference);
            long expires=input.readLong();
            if(expires<=0 || input.read()!=-1)throw new LabProtocol.Fault(LabProtocol.Reason.INVALID_FRAME);
            return new Reference(epoch,reference,expires);
        } catch(java.io.IOException error){throw new LabProtocol.Fault(LabProtocol.Reason.INVALID_FRAME);}
    }
    static byte[] channelClosed() throws LabProtocol.Fault {return wrap(0x82,new byte[]{1});}
    private LabReplies(){}
}

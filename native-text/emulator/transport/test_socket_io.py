#!/usr/bin/env python3
"""Execute actual socket owners against host Java Android/FD fakes, never SDK/AVD."""
import argparse
import os
from pathlib import Path
import re
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from owned_runtime import command

STUBS = {
    "android/system/ErrnoException.java": r'''
package android.system;
public class ErrnoException extends Exception {
    public final int errno;
    public ErrnoException(String ignored,int value) { super("fixed fake errno"); errno=value; }
    public java.io.IOException rethrowAsIOException() { return new java.io.IOException("fixed fake IO",this); }
}
''',
    "android/system/OsConstants.java": r'''
package android.system;
public final class OsConstants {
    public static final int EPERM=1,EACCES=13,EAGAIN=11,EWOULDBLOCK=11,EINPROGRESS=115,
        F_GETFL=3,F_SETFL=4,O_NONBLOCK=2048,POLLIN=1,POLLOUT=4,POLLERR=8,POLLHUP=16,POLLNVAL=32;
}
''',
    "android/system/StructPollfd.java": r'''
package android.system;
public final class StructPollfd { public java.io.FileDescriptor fd; public short events,revents; }
''',
    "android/net/Credentials.java": r'''
package android.net;
public class Credentials {
    private final int uid;
    public Credentials(int p,int u,int g) { uid=u; }
    public int getUid() { return uid; }
}
''',
    "android/net/LocalSocketAddress.java": r'''
package android.net;
public final class LocalSocketAddress {
    public enum Namespace { ABSTRACT }
    public final String name; public final Namespace namespace;
    public LocalSocketAddress(String n,Namespace scope) { name=n; namespace=scope; }
}
''',
    "android/system/Os.java": r'''
package android.system;
import java.io.*;
import java.util.*;
public final class Os {
    public static final class State {
        public int numeric,flags,reads,writes,polls,closes,uid=2000,position,readChunk=3,writeChunk=3,
            readAdvanceAfter,readAdvance,writeAdvance;
        public boolean closed,listener,eof=true,blocked,ignoreFlags,nullCredentials,closeThrows;
        public int readErrno,writeErrno,fcntlErrno;
        public byte[] input=new byte[0]; public final ByteArrayOutputStream output=new ByteArrayOutputStream();
    }
    public static final IdentityHashMap<FileDescriptor,State> states=new IdentityHashMap<FileDescriptor,State>();
    public static int nextNumeric=5,directCloses; public static boolean ignoreAllFlagSets;
    public static java.util.function.IntConsumer advance;
    public static Runnable pollHook;
    public static FileDescriptor create() {
        FileDescriptor fd=new FileDescriptor(); State state=new State(); state.numeric=nextNumeric++;
        states.put(fd,state); return fd;
    }
    public static State state(FileDescriptor fd) { State value=states.get(fd); if(value==null)throw new AssertionError("unowned fake FD");return value; }
    public static int fcntlInt(FileDescriptor fd,int cmd,int value) throws ErrnoException {
        State state=state(fd); if(state.fcntlErrno!=0)throw new ErrnoException("fixed",state.fcntlErrno);
        if(state.closed)throw new ErrnoException("fixed",9);
        if(cmd==OsConstants.F_SETFL && !state.ignoreFlags && !ignoreAllFlagSets)state.flags=value;
        return state.flags;
    }
    public static int poll(StructPollfd[] values,int timeout) throws ErrnoException {
        if(timeout<=0 || timeout>50)throw new AssertionError("unbounded fake poll");
        State state=state(values[0].fd); state.polls++;
        Runnable hook=pollHook; pollHook=null; if(hook!=null)hook.run();
        if(state.closed) { values[0].revents=(short)OsConstants.POLLNVAL; return 1; }
        if(state.blocked) { if(advance!=null)advance.accept(timeout); return 0; }
        if(state.listener) {
            if(android.net.LocalServerSocket.spuriousReady) { android.net.LocalServerSocket.spuriousReady=false; values[0].revents=(short)OsConstants.POLLIN;return 1; }
            if(!android.net.LocalServerSocket.queue.isEmpty() || android.net.LocalServerSocket.acceptUnknown) {
                values[0].revents=(short)OsConstants.POLLIN;return 1;
            }
        } else if((values[0].events&OsConstants.POLLOUT)!=0) {
            values[0].revents=(short)OsConstants.POLLOUT;return 1;
        } else if(state.position<state.input.length) {
            values[0].revents=(short)OsConstants.POLLIN;return 1;
        } else if(state.eof) { values[0].revents=(short)OsConstants.POLLHUP;return 1; }
        if(advance!=null)advance.accept(timeout); return 0;
    }
    public static int read(FileDescriptor fd,byte[] bytes,int offset,int length) throws ErrnoException,InterruptedIOException {
        State state=state(fd); state.reads++;
        if(state.closed)throw new ErrnoException("fixed",9);
        if(state.readErrno!=0) { int error=state.readErrno;state.readErrno=0;throw new ErrnoException("fixed",error); }
        if(state.position==state.input.length)return 0;
        int n=Math.min(length,Math.min(state.readChunk,state.input.length-state.position));
        System.arraycopy(state.input,state.position,bytes,offset,n); state.position+=n;
        if(state.reads==state.readAdvanceAfter && advance!=null)advance.accept(state.readAdvance);
        return n;
    }
    public static int write(FileDescriptor fd,byte[] bytes,int offset,int length) throws ErrnoException,InterruptedIOException {
        State state=state(fd);state.writes++;
        if(state.closed)throw new ErrnoException("fixed",9);
        if(state.writeErrno!=0) { int error=state.writeErrno;state.writeErrno=0;throw new ErrnoException("fixed",error); }
        int n=Math.min(length,state.writeChunk);state.output.write(bytes,offset,n);
        if(advance!=null)advance.accept(state.writeAdvance);return n;
    }
    public static void close(FileDescriptor fd) { directCloses++;throw new AssertionError("borrowed FD closed directly"); }
    public static void closeFromSocket(FileDescriptor fd) throws IOException {
        State state=state(fd);state.closes++;
        if(state.closeThrows)throw new IOException("fixed close failure"); state.closed=true;
    }
}
''',
    "android/net/LocalSocket.java": r'''
package android.net;
import java.io.*;
import android.system.*;
public class LocalSocket {
    public static int constructors,binds,connects,lazyCloseAllocations,bindErrno,connectErrno;
    public static boolean bindNoFd;
    public static String lastBind,lastConnect;
    private FileDescriptor fd; private boolean created;
    public int closeCalls;
    public LocalSocket() { constructors++; }
    public static LocalSocket accepted(int uid,byte[] input) {
        LocalSocket value=new LocalSocket();value.created=true;value.fd=Os.create();
        Os.state(value.fd).uid=uid;Os.state(value.fd).input=input;return value;
    }
    public FileDescriptor getFileDescriptor() { return fd; }
    public void bind(LocalSocketAddress address) throws IOException {
        binds++;lastBind=address.name;
        if(address.namespace!=LocalSocketAddress.Namespace.ABSTRACT)throw new AssertionError("wrong namespace");
        if(!bindNoFd) { created=true;fd=Os.create();Os.state(fd).uid=10001; }
        if(bindErrno!=0)throw new ErrnoException("fixed",bindErrno).rethrowAsIOException();
    }
    public void connect(LocalSocketAddress address) throws IOException {
        connects++;lastConnect=address.name;
        if(address.namespace!=LocalSocketAddress.Namespace.ABSTRACT || (Os.state(fd).flags&OsConstants.O_NONBLOCK)==0)
            throw new AssertionError("connect was not nonblocking/abstract");
        if(connectErrno!=0)throw new ErrnoException("fixed",connectErrno).rethrowAsIOException();
    }
    public Credentials getPeerCredentials() throws IOException {
        Os.State state=Os.state(fd);if(state.nullCredentials)return null;
        return new Credentials(1,state.uid,1);
    }
    public void close() throws IOException {
        closeCalls++;
        if(!created) { lazyCloseAllocations++;created=true;fd=Os.create(); }
        if(fd!=null) { Os.closeFromSocket(fd);fd=null; }
    }
}
''',
    "android/net/LocalServerSocket.java": r'''
package android.net;
import java.io.*;
import java.util.*;
import android.system.*;
public class LocalServerSocket {
    public static final ArrayDeque<LocalSocket> queue=new ArrayDeque<LocalSocket>();
    public static int constructors,accepts,constructorErrno;
    public static boolean spuriousReady,acceptUnknown;
    public static FileDescriptor lastAllocated;
    public static LocalServerSocket last;
    private FileDescriptor fd;
    public int closeCalls;
    public LocalServerSocket(String name) throws IOException {
        constructors++;fd=Os.create();lastAllocated=fd;Os.state(fd).listener=true;
        if(constructorErrno!=0)throw new ErrnoException("fixed",constructorErrno).rethrowAsIOException();
        last=this;
    }
    public FileDescriptor getFileDescriptor() { return fd; }
    public LocalSocket accept() throws IOException {
        accepts++;
        if(acceptUnknown) { Os.create();throw new IOException("fixed ambiguous allocation"); }
        if(queue.isEmpty())throw new ErrnoException("fixed",OsConstants.EAGAIN).rethrowAsIOException();
        return queue.remove();
    }
    public void close() throws IOException { closeCalls++;if(fd!=null) { Os.closeFromSocket(fd);fd=null; } }
}
''',
}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--java", required=True, type=Path)
    args = parser.parse_args()
    root = Path(__file__).resolve().parent
    env = {"PATH": os.defpath, "LANG": "C", "LC_ALL": "C"}
    with tempfile.TemporaryDirectory(prefix="lab-socket-host-") as directory:
        work = Path(directory)
        sources = []
        for name, value in STUBS.items():
            target = work / name
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text(value)
            sources.append(target)
        core = root / "src/org/tvcontrol/nativeinput"
        sources += [core / name for name in ("LabProtocol.java", "EditorBinding.java", "IssuedEditor.java",
                                             "LabDeadline.java", "LabSocketChannel.java", "LabSocketServer.java", "LabClientConnection.java")]
        sources.append(root / "tests/SocketIoTest.java")
        command([args.java,"-m","jdk.compiler/com.sun.tools.javac.Main","-encoding","UTF-8",
                 "-source","8","-target","8","-Xlint:-options","-d",work,*sources],env,30)
        result = command([args.java,"-cp",work,"org.tvcontrol.nativeinput.SocketIoTest"],env,10)
        if not re.fullmatch(r"PASS public socket owner host checks: [0-9]+\n", result):
            raise RuntimeError("invalid socket-owner host result")
        print(result.strip())


if __name__ == "__main__":
    main()

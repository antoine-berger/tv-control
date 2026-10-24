#!/usr/bin/env python3
"""Run actual lab pipe/client Java against host-only Android and FD fakes."""
import argparse
import importlib.util
import os
from pathlib import Path
import re
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from owned_runtime import command

spec = importlib.util.spec_from_file_location("socket_host_fakes", Path(__file__).with_name("test_socket_io.py"))
fixture = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fixture)
STUBS = {name: value for name, value in fixture.STUBS.items()
         if name not in ("android/system/Os.java", "android/net/LocalSocket.java", "android/net/LocalServerSocket.java")}
STUBS["android/os/SystemClock.java"] = r'''
package android.os;
public final class SystemClock {
    public static volatile long offset;
    public static long uptimeMillis(){return System.nanoTime()/1000000L+offset;}
}
'''
STUBS["android/system/Os.java"] = r'''
package android.system;
import java.io.*;
import java.util.*;
public final class Os {
    public static final class State {
        public int flags,reads,writes,polls,closes,readChunk=3,writeChunk=3,position,
            readErrno,writeErrno,fcntlErrno,readAdvanceAfter,readAdvance,writeAdvance;
        public boolean eof,closed,blocked,ignoreFlags;
        public byte[] input=new byte[0];
        public final ByteArrayOutputStream output=new ByteArrayOutputStream();
    }
    public static final IdentityHashMap<FileDescriptor,State> states=new IdentityHashMap<FileDescriptor,State>();
    public static int uid=2000,euid=2000,directCloses;
    public static volatile FileDescriptor connected;
    public static volatile Runnable writeHook,pollHook;
    public static volatile java.util.function.Consumer<FileDescriptor> pollEach;
    public static java.util.function.IntConsumer advance;
    public static int getuid(){return uid;}public static int geteuid(){return euid;}
    public static synchronized FileDescriptor create(){FileDescriptor fd=new FileDescriptor();states.put(fd,new State());return fd;}
    public static synchronized State state(FileDescriptor fd){State s=states.get(fd);if(s==null)throw new AssertionError("unknown fake FD");return s;}
    public static synchronized void reset(){
        states.clear();states.put(FileDescriptor.in,new State());states.put(FileDescriptor.out,new State());
        uid=euid=2000;directCloses=0;connected=null;writeHook=null;pollHook=null;pollEach=null;advance=null;
    }
    public static void append(FileDescriptor fd,byte[] bytes){State s=state(fd);synchronized(s){
        byte[] next=Arrays.copyOf(s.input,s.input.length+bytes.length);System.arraycopy(bytes,0,next,s.input.length,bytes.length);s.input=next;
    }}
    public static void eof(FileDescriptor fd){State s=state(fd);synchronized(s){s.eof=true;}}
    public static byte[] output(FileDescriptor fd){State s=state(fd);synchronized(s){return s.output.toByteArray();}}
    public static int fcntlInt(FileDescriptor fd,int cmd,int value)throws ErrnoException {
        State s=state(fd);synchronized(s){
            if(s.closed || s.fcntlErrno!=0)throw new ErrnoException("fixed",s.closed?9:s.fcntlErrno);
            if(cmd==OsConstants.F_SETFL && !s.ignoreFlags)s.flags=value;return s.flags;
        }
    }
    public static int poll(StructPollfd[] values,int timeout)throws ErrnoException {
        if(timeout<=0 || timeout>50)throw new AssertionError("unbounded poll");
        Runnable hook=pollHook;pollHook=null;if(hook!=null)hook.run();
        java.util.function.Consumer<FileDescriptor> each=pollEach;if(each!=null)each.accept(values[0].fd);
        State s=state(values[0].fd);synchronized(s){
            s.polls++;
            if(s.closed){values[0].revents=(short)OsConstants.POLLNVAL;return 1;}
            if(!s.blocked){
                if((values[0].events&OsConstants.POLLOUT)!=0){values[0].revents=(short)OsConstants.POLLOUT;return 1;}
                if(s.position<s.input.length){values[0].revents=(short)OsConstants.POLLIN;return 1;}
                if(s.eof){values[0].revents=(short)OsConstants.POLLHUP;return 1;}
            }
        }
        if(advance!=null)advance.accept(timeout);
        else try{Thread.sleep(Math.min(timeout,2));}catch(InterruptedException error){Thread.currentThread().interrupt();throw new ErrnoException("fixed",4);}
        return 0;
    }
    public static int read(FileDescriptor fd,byte[] bytes,int offset,int length)throws ErrnoException,InterruptedIOException {
        State s=state(fd);synchronized(s){
            s.reads++;if(s.closed)throw new ErrnoException("fixed",9);
            if(s.readErrno!=0){int n=s.readErrno;s.readErrno=0;throw new ErrnoException("fixed",n);}
            if(s.position==s.input.length){if(s.eof)return 0;throw new ErrnoException("fixed",OsConstants.EAGAIN);}
            int n=Math.min(length,Math.min(s.readChunk,s.input.length-s.position));
            System.arraycopy(s.input,s.position,bytes,offset,n);s.position+=n;
            if(s.reads==s.readAdvanceAfter && advance!=null)advance.accept(s.readAdvance);return n;
        }
    }
    public static int write(FileDescriptor fd,byte[] bytes,int offset,int length)throws ErrnoException,InterruptedIOException {
        State s=state(fd);int n;synchronized(s){
            s.writes++;if(s.closed)throw new ErrnoException("fixed",9);
            if(s.writeErrno!=0){int value=s.writeErrno;s.writeErrno=0;throw new ErrnoException("fixed",value);}
            n=Math.min(length,s.writeChunk);s.output.write(bytes,offset,n);
            if(advance!=null)advance.accept(s.writeAdvance);
        }
        Runnable hook=writeHook;if(hook!=null)hook.run();return n;
    }
    public static void close(FileDescriptor fd){directCloses++;throw new AssertionError("borrowed close");}
    public static void closeFromSocket(FileDescriptor fd)throws IOException {
        State s=state(fd);synchronized(s){s.closes++;s.closed=true;}
    }
}
'''
STUBS["android/net/LocalSocket.java"] = r'''
package android.net;
import android.system.*;
import java.io.*;
public class LocalSocket {
    public static int constructors,binds,connects,closes,peerUid=10001,bindErrno,connectErrno;
    public static boolean closeFailure;
    public static Runnable onConnect;
    private FileDescriptor fd;
    public LocalSocket(){constructors++;}
    public FileDescriptor getFileDescriptor(){return fd;}
    public void bind(LocalSocketAddress address)throws IOException {
        binds++;fd=Os.create();
        if(address.namespace!=LocalSocketAddress.Namespace.ABSTRACT)throw new AssertionError("namespace");
        if(bindErrno!=0)throw new ErrnoException("fixed",bindErrno).rethrowAsIOException();
    }
    public void connect(LocalSocketAddress address)throws IOException {
        connects++;
        if((Os.state(fd).flags&OsConstants.O_NONBLOCK)==0)throw new AssertionError("blocking connect");
        if(connectErrno!=0)throw new ErrnoException("fixed",connectErrno).rethrowAsIOException();
        Os.connected=fd;Runnable hook=onConnect;if(hook!=null)hook.run();
    }
    public Credentials getPeerCredentials(){return new Credentials(1,peerUid,1);}
    public void close()throws IOException {
        closes++;if(closeFailure)throw new IOException("fixed");
        if(fd==null)throw new AssertionError("lazy allocation attempted");Os.closeFromSocket(fd);fd=null;
    }
    public static void reset(){constructors=binds=connects=closes=0;peerUid=10001;bindErrno=connectErrno=0;closeFailure=false;onConnect=null;}
}
'''


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--java", required=True, type=Path)
    args = parser.parse_args()
    root = Path(__file__).resolve().parent
    env = {"PATH": os.defpath, "LANG": "C", "LC_ALL": "C"}
    with tempfile.TemporaryDirectory(prefix="lab-client-host-") as directory:
        work = Path(directory)
        sources = []
        for name, value in STUBS.items():
            target = work / name
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text(value)
            sources.append(target)
        core = root / "src/org/tvcontrol/nativeinput"
        sources += [core / name for name in ("LabProtocol.java", "EditorBinding.java", "IssuedEditor.java",
                                             "LabReplies.java", "LabDeadline.java", "LabSocketChannel.java",
                                             "LabClientConnection.java", "LabPipeIo.java", "LabSocketClient.java")]
        sources.append(root / "tests/SocketClientTest.java")
        command([args.java, "-m", "jdk.compiler/com.sun.tools.javac.Main", "-encoding", "UTF-8",
                 "-source", "8", "-target", "8", "-Xlint:-options", "-d", work, *sources], env, 30)
        result = command([args.java, "-cp", work, "org.tvcontrol.nativeinput.SocketClientTest"], env, 15)
        if not re.fullmatch(r"PASS private client host checks: [0-9]+\n", result):
            raise RuntimeError("invalid private-client host result")
        print(result.strip())


if __name__ == "__main__":
    main()

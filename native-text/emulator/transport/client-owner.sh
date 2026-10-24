#!/system/bin/sh
# Owned lab only. No payload/ref/cache token in arguments or environment.
# No set -e: the remote wait witness is required even for a failed child.
if [ "$#" -ne 3 ]; then exit 64; fi
lab_instance=$1
client_artifact=$2
expected_uid=$3
case "$lab_instance" in ''|*[!0-9a-f]*) exit 64;; esac
if [ "${#lab_instance}" -ne 32 ]; then exit 64; fi
case "$expected_uid" in ''|0*|*[!0-9]*) exit 64;; esac
if [ "${#expected_uid}" -gt 10 ] || [ "$expected_uid" -gt 2147483647 ] || [ "$expected_uid" -eq 2000 ]; then exit 64; fi
case "$client_artifact" in *[!A-Za-z0-9_./~+=-]*|*/../*|*/./*) exit 64;; esac
case "$client_artifact" in
  /data/app/*/base.apk|/data/local/tmp/tv-native-lab-"$lab_instance"/client.jar) ;;
  *) exit 64;;
esac

# Foreground execution preserves stdin and the shell waits for this exact child.
# There is no background stdin=/dev/null rule, trap, PID scan or later PID signal.
CLASSPATH="$client_artifact" /system/bin/app_process /system/bin org.tvcontrol.nativeinput.LabSocketClient "$lab_instance" "$expected_uid" <&0
client_exit=$?

# Only after the child has exited/reaped: frame0x83, instance16, exit class0/1.
# All printf bytes derive from constants and validated PUBLIC rendezvous metadata.
marker='\000\000\000\027\116\124\114\061\001\203'
rest=$lab_instance
while [ -n "$rest" ]; do
  tail=${rest#??}
  pair=${rest%"$tail"}
  rest=$tail
  value=$((0x$pair))
  octal1=$((value / 64))
  octal2=$(((value / 8) % 8))
  octal3=$((value % 8))
  marker="${marker}\\${octal1}${octal2}${octal3}"
done
if [ "$client_exit" -eq 0 ]; then marker="${marker}\000"; else marker="${marker}\001"; fi
/system/bin/toybox printf "$marker"
marker_exit=$?
if [ "$marker_exit" -ne 0 ]; then exit "$marker_exit"; fi
exit "$client_exit"

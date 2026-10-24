# Future transport options — assessment only

No production transport is implemented or authorized for a user device. The
current runtime proof creates synthetic request files inside the test app's own
cache and tests the exact shared engine. That establishes neither a complete host
integration nor permission to install a helper on the user's TV.

## Standard Instrumentation Bundle extras

`am instrument -e <key> <value>` is a public supported route to Instrumentation
Bundle arguments. A strictly length-capped, validated base64 frame could transport
the existing binary request without putting arbitrary shell syntax into a command.
It is therefore incorrect to call helper input intrinsically impossible merely
because ordinary shell cannot write the app-private cache.

However, encoding is not encryption or privacy. The frame can be exposed in host
or device process arguments, shell/service diagnostics, instrumentation logs or
crash reporting. Avoiding shell injection does not satisfy the existing private-
payload requirement. Rejecting password/visible-password editors is necessary
but insufficient: a normal search field can still contain sensitive text.

This option must not silently become the default. A future deliberately selected
non-sensitive mode would require an explicit privacy decision, bounded strict
base64/binary validation, trusted issued-reference/session binding, replay
prevention, cancellation/deadline semantics and stable status-only reporting.
Those checks are not present merely because a Bundle transports bytes. All
password/secret payloads would remain excluded; no claim that code can infer
non-sensitivity from field type is acceptable.

For an explicitly reviewed emulator-only experiment, public synthetic strings
could use this route to test the Binder/Instrumentation entrypoint. Such a test
would not approve it for user text or weaken the private transport requirement.
No new Bundle payload option is implemented in this candidate.

## Private payload route

The app-private-cache design keeps text out of argv, but still needs a reviewed
host-to-helper delivery mechanism. It is an unresolved implementation question,
not evidence that no mechanism exists. Do not add debug/run-as privileges,
exported providers/receivers, new permissions or a persistent listener as an
unreviewed shortcut. A small standard supported route should be assessed before
building a new framework. Any materially different privacy or installation scope
requires approval first.

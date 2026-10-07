# Local network access on macOS

macOS 15 and later require permission for local-network connections. This can
affect terminal commands such as `kubectl get nodes`, SSH, database connections
and remote files when their servers are on a local network.

## Request access

1. Open **Kiln → Settings → Behavior**.
2. Under **Privacy → Local Network**, choose **Request Access**.
3. If macOS displays its permission prompt, allow Kiln, then retry the command.

The gear button opens **System Settings → Privacy & Security**. Choose
**Local Network** there and check Kiln. Kiln cannot inspect or change the system's
permission decision. An attempted request is not confirmation that access was
granted. If no suitable IPv6 link-local interface is available, the request
cannot be initiated with this mechanism; the notice says so without claiming
that your network is disconnected.

The request runs only when you choose the button. It uses Apple's documented
best-effort UDP `connect` approach without sending packets or discovering devices.
The system may not display a new prompt if a decision already exists.
The native permission description follows your macOS language; Kiln supports
English, Korean, Japanese and Simplified Chinese descriptions.

## Background terminals

The terminal owner can survive the GUI that originally launched it. On macOS,
new commands in that owner can then lose their attribution to the Kiln app,
even when Kiln's Local Network switch is enabled. Terminal.app succeeding while
Kiln reports `no route to host` does not establish a missing network route.

On macOS, newly opened panels use a separate persistent PTY host. Local checks
of the signed app reproduced the old daemon's failure and returned node data
from the same cluster through new hosted panels. Retained native panels keep
their original owner so an update does not terminate existing work.

Kiln initializes its daemon and PTY hosts as their own responsible processes
before restoring or creating terminals. This replaces the same signed executable
with the same PID and preserves inherited restore descriptors and child-exit
ownership. It does not change consent, another app's identity or system settings.

This uses dynamically resolved `responsibility_spawnattrs_setdisclaim`, a private
macOS SPI also used by [Chromium's launcher](https://chromium.googlesource.com/chromium/src/+/main/base/process/launch_mac.cc),
with the public [SETEXEC replacement flag](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man3/posix_spawnattr_getflags.3.html).
If the SPI is unavailable or fails, Kiln reports a startup warning and preserves
the existing session startup path. This is not a permission-status API.

**Existing shells are preserved, but their old responsibility chain is not
retroactively repaired.** After updating, open a new terminal panel and retry the
command there. Finish work in an affected older panel before closing it; updating
does not automatically restart its shell or agent.

## Maintainer checks

The GUI (`dev.kiln.app`) and menu companion (`dev.kiln.statusbar`) must have
distinct executable build UUIDs. Build both binaries from their distinct entry
points; do not copy or rename the GUI executable as the companion.
`scripts/configure-network-privacy.py` writes descriptions before signing and
refuses missing, colliding or mismatched-architecture UUIDs. Verify extracted
release bundles without changing signed resources:

```sh
python3 scripts/test-network-privacy.py
python3 scripts/configure-network-privacy.py --verify /path/to/Kiln.app
```

Code tests cover interface selection, scope preservation, explicit clicks and
neutral results. They do not establish a native permission prompt or attribution
for terminal children. On a fresh macOS user account, launch the final signed app
from Finder, request access and retry a controlled local endpoint. Repeat after
opening the GUI from the menu companion. Also test an upgrade with retained
daemon/PTY processes. Keep existing sessions intact; do not reset TCC, disable
privacy protections or run the app as root to make a test pass.

Run `cargo test --locked -p kiln-daemon --test macos_responsibility` for same-PID
replacement, raw environment/arguments, inherited restore descriptors, new-child
attribution and child-exit ownership. These checks make no network requests and
do not prove native approval or denial. Native release verification must compare
retained old panels and new panels separately, including native and hosted PTYs;
check that denial still denies access in a separate user-controlled account.

References: [Apple local network privacy](https://developer.apple.com/documentation/technotes/tn3179-understanding-local-network-privacy),
[usage description](https://developer.apple.com/documentation/bundleresources/information-property-list/nslocalnetworkusagedescription),
[distinct executable UUIDs](https://developer.apple.com/documentation/technotes/tn3178-checking-for-and-resolving-build-uuid-problems).

Hosted handover tests include blocked stdin, failed state writes and execution,
unchanged foreground jobs, and mixed native/hosted upgrades. A mixed upgrade keeps
the original daemon PID and native child ownership; completed hosted history is
retained and one unreachable host cannot abort restoration of native panels.
A stalled legacy host cancels the upgrade after three seconds rather than holding
the lifecycle lock indefinitely. These checks do not establish user consent.

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

References: [Apple local network privacy](https://developer.apple.com/documentation/technotes/tn3179-understanding-local-network-privacy),
[usage description](https://developer.apple.com/documentation/bundleresources/information-property-list/nslocalnetworkusagedescription),
[distinct executable UUIDs](https://developer.apple.com/documentation/technotes/tn3178-checking-for-and-resolving-build-uuid-problems).

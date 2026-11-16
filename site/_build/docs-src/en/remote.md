---
title: Remote files and SSH
description: Browse and edit files on Amazon S3, Google Cloud Storage, OCI Object Storage, Cloudflare R2, FTP/FTPS and SFTP, and open SSH terminals from your SSH config.
---

Kiln browses object storage, FTP servers and SFTP hosts in the workspace inspector, next to your terminals, databases and local files. Remote files open in the same editor as local ones, and SSH hosts from your OpenSSH config are ready to pick.

## Add a connection {#add-a-connection}

Choose **Tools → Remote connections** in the titlebar, then add a named connection. The tool opens alongside your terminal task without replacing it.

| Type | What you provide |
| --- | --- |
| Amazon S3, Google Cloud Storage, OCI Object Storage, Cloudflare R2 | Authentication, then a bucket |
| FTP / FTPS | Host, port and username |
| SFTP / SSH | A host from **SSH Config host**, or **Enter manually** |

Saved connection settings contain names and addresses. Passwords and keys go to Kiln's credential store (the macOS Keychain on a Mac). See [Credentials](remote.html#credentials).

## Object storage {#object-storage}

Choose authentication first, then the bucket. Once authentication and any required scope are ready, Kiln loads the list of buckets in the background. If there is only one, it is selected for you.

| Provider | Authentication and scope |
| --- | --- |
| **AWS S3** | **Saved AWS profile**, **Default AWS authentication**, or **Enter keys manually**. Listing buckets needs permission to list the account's buckets. |
| **Google Cloud Storage** | Native authentication lists the project you specify. The selected CLI configuration's project is filled in when available. |
| **OCI Object Storage** | Native authentication needs a compartment OCID. The profile's region and namespace settings stay in effect. |
| **Cloudflare R2** | Native authentication uses the account you specify. The list covers the default jurisdiction. |
| **S3-compatible** | Uses that service's bucket-list API. What you see depends on the service and your credentials. |

If your credentials can reach a known bucket but cannot list buckets, use **Enter manually** for that bucket. A successful bucket list does not prove you can write to it.

Kiln never creates buckets, signs you in or changes your cloud credentials while it looks them up.

### AWS profiles {#aws-profiles}

With **Saved AWS profile**, Kiln reads profiles from `~/.aws/config` and `~/.aws/credentials`, and follows `AWS_CONFIG_FILE` and `AWS_SHARED_CREDENTIALS_FILE` if you set them. Kiln saves the profile name, not a copy of its keys, and never modifies your AWS files. A profile's region fills the region field until you edit it.

For profiles that use SSO, roles or `credential_process`, install AWS CLI v2 and complete the profile's usual AWS login first. Kiln resolves the credentials through the CLI when it connects. If the selected profile is unavailable or expired, you get an error. Kiln does not fall back to another account.

Endpoint, starting prefix and path-style addressing are under **Advanced connection settings**.

### Reusable authentication {#reusable-authentication}

To use the same identity for several buckets, expand **Save as authentication profile**, give it a name and save. No bucket is needed. Select the profile again later, rename it by saving under a new name, or delete it.

Each saved bucket connection keeps its own copy of the secret it needs, so renaming or deleting a reusable profile does not break existing connections.

## FTP and FTPS {#ftp-and-ftps}

Enter the host, port and username. FTPS uses explicit TLS and validates the server certificate.

> **Warning:** Plain FTP sends everything unencrypted, including your password. Use it only when your server requires it.

## SFTP and SSH hosts {#sftp-and-ssh-hosts}

Pick a host from the **SSH Config host** dropdown. Kiln reads `~/.ssh/config` and its static `Include` files, and fills in the host, username, port and key path. To use another config file, choose it under **Advanced connection settings**. To connect by hostname or IP address without an SSH alias, choose **Enter manually**.

Kiln only reads your SSH config. It runs no SSH commands, `Match exec`, authentication or network connections, so conditional settings are resolved by OpenSSH when you connect.

- A selected host keeps its alias, jump hosts and other OpenSSH settings.
- Fields you leave untouched follow your config at connection time.
- Fields you edit become overrides, shared by SFTP and **Open SSH terminal**. Clear an override to follow the config again.
- A key path adds an identity for OpenSSH. It does not restrict the connection to that key.

### SSH terminals and host keys {#ssh-terminals-and-host-keys}

**Open SSH terminal** opens a terminal to the same host, using the same overrides as SFTP. The SFTP browser uses OpenSSH key or agent authentication and needs a known host key. Kiln never turns off host-key verification.

For a new server:

1. Open its **SSH terminal**.
2. Check and accept the server fingerprint through OpenSSH.
3. Reconnect the file browser.

Password-only hosts work in the SSH terminal. The SFTP browser needs key or agent authentication that runs without prompts.

## Browse and edit {#browse-and-edit}

Select a saved connection to browse it in the inspector. The connection shows its bucket or host and the current path.

- Double-click a folder to enter it, and use the up arrow to go back. The up control is disabled at the starting directory.
- Use the path field, filter and refresh controls beside the file list. S3 listings show **Load more** when another page is available.
- Upload, download, rename, delete and create folders from the toolbar or the context menu.

Open a file to edit it in its own tab in the main workspace, with the same [editor](editor.html) as local files. Two remote files with the same name keep separate tabs. Browsing folders never replaces your terminal or the file you are editing.

| Action | What it does |
| --- | --- |
| **Save remotely** | Checks that the remote file still matches the version you opened, then uploads your changes |
| **Keep local copy** | Saves a copy without writing to the server |

Remote editing is limited to 8 MB. Download larger or non-text files and work on them locally. Unsaved remote text is kept with Kiln's saved work, and restoring it never uploads anything on its own. If you change a connection's settings after opening a file, remote saves for that file are blocked and your local draft stays available.

To pick up changed connection settings, choose **Reconnect**. It reloads the saved profile and credentials and returns to the starting directory. Reconnect is unavailable while you are editing or transferring.

## Transfers {#transfers}

Transfers run in the background and keep going while the inspector is hidden. Kiln shows their progress, and you can cancel them.

- Upload several files with the picker or by dropping them on the file list. They transfer in order, and Kiln asks before replacing existing files.
- Cancelling stops the rest of the upload queue. The current step may take a moment to finish.
- Downloads replace the local file only after they complete.
- Only empty folders can be deleted. There is no hidden recursive delete.
- After a cancel or a server error, refresh to see the actual remote state.

Wait for transfers to finish, or cancel them, before you close the workspace or quit. A transfer recovered after a restart is a notice to check its result. Kiln never retries it automatically.

### S3 limits {#s3-limits}

- Uploads and renames support objects up to 5 GiB.
- Folder prefixes cannot be renamed.
- Rename is a copy followed by a delete. The source is deleted only after the copy succeeds.

## Credentials {#credentials}

Edit a saved connection to replace its credentials.

- In manual-key mode, empty fields keep the saved keys. Enter the access key and secret key together to replace them.
- Replacing S3 keys clears the old session token unless you enter a new one under **Temporary session token**.
- Switching to a saved profile or default AWS credentials removes the manual keys Kiln stored.
- FTP connections offer **Remove saved credentials** in the edit form.
- Changing the connection type clears the earlier protocol's credentials.

## Windows {#windows}

- SFTP and SSH use Windows OpenSSH, and `C:\…` paths in your SSH config work as written.
- Cloud CLIs installed as `.cmd` shims are found through `PATHEXT`, and the gcloud and Wrangler folders follow `%APPDATA%`.

## Kiln Remote for phones {#mobile-companion}

Kiln Remote 0.1.0 is an optional Android/iPhone companion built separately from desktop Kiln 0.3.0. Start the host manually on your computer, allow specific workspaces and pair using its per-launch URL and token. Closing the desktop window does not start the host. Mobile packages are built from source; there is no App Store or Play Store download in this stage.

Open or create native conversations, explicitly send tasks, inspect tool output and answer complete approvals. Commands insert an editable draft. Supported controls show their native scope and pending or confirmed state; unsupported controls remain read-only. Creating a task keeps its prompt as a draft until you press Send. Connection loss never automatically repeats a prompt. Credentials and drafts stay in memory; explicit Disconnect clears the connection, and closing the app loses unsaved memory-only data.

For phones, the owner must provision signing and a trusted HTTPS connection to the loopback host. Emulator/Simulator checks do not establish physical-device or remote-network acceptance. See the [build, connection and verification guide](https://github.com/YangTaeyoung/kiln/blob/main/docs/mobile-remote.md) for exact steps, current evidence and limits. Desktop screenshots on this site retain their original provenance and are not mobile screenshots.

### Verification on 2026-11-16 {#mobile-verification}

| Scope | Observed evidence and remaining checks |
| --- | --- |
| Source | Historical baseline `e94f490`; dark fix `c078e58`; attachment draft fix `f0b380f3f8b6ca154c6764b47f3f179847216646`. Desktop source and capture provenance remain separate. |
| Android and iOS native execution | Create, explicit Send, installed Claude 2.1.325, real stdio MCP, one-time Allow and exact completed echo each produced one effect at `b45beac`. Inference used a synthetic loopback service. Later app fixes do not constitute a repeated final-head MCP run. |
| Settings and width | At `e94f490`, both actual UIs selected Effort Level high; independent GET confirmed Applied, effective high and settled=true. iOS input width was 402px in a 402px frame; Android viewport and scroll width were both 360 CSS px. |
| Redirects and terminals | At `e94f490`, both native apps refused redirects: two source requests, zero destination requests and zero forwarded authorization. Android at `b45beac` and iOS at `e94f490` explicitly sent terminal input + Enter to a real private shell and produced the expected effect once per platform. |
| Drafts and images | Both actual apps preserved text after Disconnect/re-pair at `e94f490`, without extra MCP effects. Android uploaded a 6,403-byte PNG through its native picker and displayed a chip. Bounded iOS automation did not observe the Photo Library handoff; actual iOS upload remains unverified. |
| Attachment draft fix | Images now survive settings, commands/skills, task-list navigation and same-live-host re-pairing. Images from another address or host launch show Unavailable and block Send until removed and explicitly selected/uploaded again. No automatic retry. Ten real component regressions cover navigation, session boundaries and late receipts; the full app suite passed 47 tests. Native acceptance of this later fix remains pending. |
| Dark appearance and CI | Both rebuilt native apps were inspected as readable at `c078e58`; rendered contrast regression passed ≥4.5 after reproducing 1.095 before the fix. All three mobile CI jobs passed at that source, including development/production builds. Final attachment-source CI/artifacts remain pending. These observations do not certify every route, width or keyboard state. |
| Owner acceptance and cleanup | Physical phones, signing, Korean IME and trusted HTTPS remain owner tasks, including when using SSH forwarding. Desktop exact-owned cleanup is complete except the existing three normal Antigravity history cleanup boundaries; mobile processes/devices/data and the final audit remain pending. Original app, daemon, auth/preferences and protected desktop release remain preserved. |

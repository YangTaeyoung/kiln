# Remote files and SSH

Kiln can browse S3 buckets, FTP/FTPS servers and SFTP hosts alongside your
terminals, databases and local files. Saved connection settings contain names
and addresses; passwords and S3 credentials use the account credential store
(macOS Keychain).

## Connect

Open **Remote connections** in the tools sidebar, then add a named connection:

- **S3:** bucket and region, then choose **Saved AWS profile**, **Default AWS
  credentials**, or **Enter keys manually**. Saved profiles come from
  `~/.aws/config` and `~/.aws/credentials` (including `AWS_CONFIG_FILE` and
  `AWS_SHARED_CREDENTIALS_FILE` overrides). Kiln saves the selected profile name,
  not a copy of its keys. A profile's region fills the region field until you edit
  it yourself. Endpoint, starting prefix and path-style addressing are under
  **Advanced connection settings**.
- **FTP / FTPS:** host, port and username. FTPS uses explicit TLS and validates
  the server certificate. FTP transfers, including the password, are unencrypted;
  use it only when your server requires it.
- **SFTP:** an SSH host alias. Import aliases from `~/.ssh/config` or choose a
  configuration file. Includes and literal aliases are discovered without
  executing SSH commands. OpenSSH resolves the full configuration when you
  explicitly connect, including keys, agents and jump hosts.

SFTP uses OpenSSH key/agent authentication and requires a known host key. For a
new server, open its **SSH terminal**, inspect and accept the server fingerprint
through OpenSSH, then reconnect the file browser. Kiln never disables host-key
verification. Password-only SSH hosts can be used in the SSH terminal; the
background SFTP browser requires noninteractive key or agent authentication.

For S3 profiles backed by SSO, roles or `credential_process`, install AWS CLI v2
and complete the profile's normal AWS login first. Kiln resolves these credentials
with the CLI when connecting; merely opening the profile selector does not run
authentication. An unavailable or expired selected profile produces an error
instead of connecting through another account. Profile discovery never modifies
your AWS files. See [AWS profile settings](https://docs.aws.amazon.com/cli/latest/userguide/cli-configure-files.html)
and [credential export](https://docs.aws.amazon.com/cli/latest/reference/configure/export-credentials.html).

Edit a saved connection to replace credentials. In manual-key mode, empty fields
retain saved keys; enter access and secret keys together when replacing them.
Replacing S3 keys clears the old session token unless you enter a new token under
**Temporary session token**. Switching to a saved profile or default AWS
credentials removes Kiln's stored manual keys. FTP provides **Remove saved
credentials** in its edit form. Changing connection type also clears credentials
from the earlier protocol.

## Browse, transfer and edit

Open a saved connection to get a central file tab. Double-click folders to
navigate, use the path field or go up one level, and filter the current listing.
S3 listings have **Load more** when another page is available. The parent control
is disabled at the configured starting directory. To change saved connection
settings, choose **Reconnect** from the file tab menu; this reloads the saved
profile and credentials and returns to its starting directory. Reconnection
is unavailable while editing or transferring.

Use the toolbar or context menu to upload, download, rename, create folders and
delete files. Upload multiple files through the picker or drop them into the
file list; they transfer in order, with confirmation for existing listed files.
Cancelling a transfer stops the remaining upload queue. S3 folder prefixes do
not offer rename.
Only empty folders can be deleted; there is no hidden recursive deletion.

Double-click a file to edit it with the existing language-aware editor. Remote
editing is limited to 8 MB; larger files can be downloaded and edited locally.
**Save remotely** checks that the remote bytes still match the version you
opened, then uploads your changes. **Keep locally** saves a copy without writing
to the server. Unsaved remote text is retained in Kiln's ordinary saved work;
restoring it does not automatically upload anything. Clean editing files and
completed comparison downloads are removed from Kiln's private cache; unsaved
drafts remain available for recovery.

## Transfer behavior

Operations run on background workers. Transfer progress and cancellation are
shown in the file tab. Downloads replace their local destination only after a
complete download. FTP/SFTP uploads stage a temporary file and then rename it;
S3 uploads use bounded multipart transfers, then publish the complete object.
A cancelled operation can take until the current socket operation finishes.
Always refresh after a cancellation or a server error to inspect the actual
remote result.

S3 staged uploads and object rename currently support objects up to 5 GiB.
S3 folder prefixes cannot be renamed as a single filesystem operation. S3 rename is
copy followed by delete; it is not a filesystem transaction. Source deletion
only follows a confirmed successful copy. S3 uses a conditional destination
request when replacement is not approved. Other S3-compatible services must
support that condition themselves. FTP/SFTP replacement checks and remote-edit
checks cannot lock out a second client that writes between the check and the
final publish. They do not provide a cross-client conflict-free guarantee.

## Implementation references

- [OpenSSH SFTP commands and batch authentication](https://man.openbsd.org/sftp)
- [OpenSSH configuration](https://man.openbsd.org/ssh_config)
- [S3 CopyObject: limits, conditions and embedded errors](https://docs.aws.amazon.com/AmazonS3/latest/API/API_CopyObject.html)
- [rust-s3](https://github.com/durch/rust-s3)
- [SuppaFTP](https://github.com/veeso/suppaftp)

Developers can run the isolated protocol tests described in
[remote-file verification](maintainers/remote-files-verification.md).

# Object storage connections

Open **Tools → Remote connections**, add a connection, and choose Amazon S3,
Google Cloud Storage, OCI Object Storage, or Cloudflare R2.

Choose authentication before selecting a bucket. Kiln reads the bucket catalog
on a worker after the authentication and required scope are ready. A catalog
with one bucket is selected automatically; multiple buckets require a choice.
Changing authentication or scope discards the old catalog and cancels its job.
Refresh retains a choice only if it is still present in the returned catalog.

- **AWS S3:** select an AWS CLI profile, default AWS authentication, or manual
  S3 credentials. Listing requires permission to list the account's buckets;
  this is distinct from permission to operate on an individual bucket.
- **Google Cloud Storage:** native authentication lists the specified project.
  The selected CLI configuration's project is prefilled when available.
- **OCI Object Storage:** native authentication requires a compartment OCID.
  The profile's configured region and namespace behavior remain in effect.
- **Cloudflare R2:** native authentication uses the specified account. The
  catalog covers the default jurisdiction supported by this connection type.
- **S3-compatible authentication:** uses that service's S3 bucket-list API.
  Its permissions and catalog scope depend on the service and credentials.

Use **Direct input** when credentials can access a known bucket but cannot list
buckets. A successful catalog lookup does not prove write access. Kiln does not
create buckets, sign in, or modify cloud credentials while discovering them.

## Browse and edit

Select a saved connection to browse its folders and files **in the inspector**.
The connection shows its bucket or host and current path, rather than only the
protocol name. Double-click a folder to enter it; use the up arrow to return. The path
field, filter, refresh and pagination controls stay beside the file list.

Click a file to open its own editor tab in the main workspace. Two different
remote paths retain separate tabs, including when their filenames match. Folder
navigation does not replace the terminal or a file being edited. Upload, download,
rename, delete and folder creation remain in the inspector toolbar/context menus.
Large or non-text files can be downloaded instead of opened in the text editor.

Transfers keep progressing while the inspector is hidden. Wait for completion or cancel a
pending transfer before closing its workspace or quitting; unrelated terminal
panels can still be closed. A recovered transfer is a notice to inspect its result,
never an automatic retry. Saved file edits retain their original connection
snapshot, and changes to that connection block subsequent remote saves while
keeping the local draft available. Requests already dispatched keep their original
target and are not undone by changing connection settings.

Older directory tabs move to inspector navigation on restore. Unsaved remote
file edits continue to open as file editors with their original path and content.
This separation follows [VS Code's remote Explorer/editor
workflow](https://code.visualstudio.com/docs/remote/ssh) and [Transmit's remote
file editing actions](https://help.panic.com/transmit/transmit5/double-click/).

## Reusable authentication

Expand **Save as authentication profile**, give the identity a name, and save.
A bucket and connection name are not required. This stores authentication
metadata separately from the connection's bucket and starting path. The profile
can be selected again, renamed by saving a new name, or deleted with confirmation.
Manual access keys and tokens are kept in the credential store, never the JSON
metadata file. Empty credential fields retain the selected profile's saved keys.

Saving a bucket connection copies its required secret to that connection's own
credential-store record. Renaming or deleting the reusable profile therefore
does not invalidate existing connections. CLI profiles retain the exact saved
profile/configuration reference rather than copying transient access tokens.

## Design references and verification boundary

The connection/authentication distinction follows [Transmit's saved connection
flow](https://help.panic.com/transmit/transmit5/servers/). Bucket browsing and a
known-bucket fallback follow [Cyberduck's S3 connection
model](https://docs.cyberduck.io/protocols/s3/). Discovery scope follows the
[AWS ListBuckets API](https://docs.aws.amazon.com/AmazonS3/latest/API/API_ListBuckets.html),
[GCS bucket-list API](https://docs.cloud.google.com/storage/docs/json_api/v1/buckets/list),
[OCI bucket-list command](https://docs.oracle.com/en-us/iaas/tools/oci-cli/latest/oci_cli_docs/cmdref/os/bucket/list.html),
and [Cloudflare R2 API](https://developers.cloudflare.com/api/resources/r2/).

Tests use temporary metadata, an in-memory credential store, injected catalogs,
and provider loopback/CLI fixtures. These demonstrate local state and request
contracts; they do not certify access to a user's live cloud account. See
[remote verification](maintainers/remote-files-verification.md) for wider limits.

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

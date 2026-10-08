# Object storage

Open **Tools → Remote files**, add a connection, and choose Amazon S3,
Google Cloud Storage, Oracle Object Storage, or Cloudflare R2. Each connection
keeps its bucket, starting prefix, and selected authentication source. Folders
are object-name prefixes; creating an empty folder writes a zero-byte `/` marker.
Deleting a folder requires it to be empty and never recursively deletes objects.

## Existing CLI profiles

| Provider | Discovered source | Authentication during an operation |
| --- | --- | --- |
| S3 | AWS shared configuration/credentials | Existing AWS profile resolver, including role and SSO credentials |
| Oracle | Sections in `OCI_CLI_CONFIG_FILE` or `~/.oci/config`, with the official `OCI_CONFIG_FILE` / `~/.oraclebmc/config` fallback | Installed `oci`, with an explicit profile and configuration file |
| Google | Named configurations in `~/.config/gcloud` or `CLOUDSDK_CONFIG`; a separate ADC choice when the file exists | Installed `gcloud`, with an explicit configuration or ADC file, then the Cloud Storage JSON API |
| Cloudflare | Wrangler's `config/<profile>.toml` and encrypted `.enc` profile files | Installed `wrangler auth token --json --profile=…`, then the R2 object API |

Profile discovery reads names and non-secret configuration metadata. It does not
run login commands, inspect private key/token files, test cloud permissions, or
change the active CLI profile. Profiles remain visible when their CLI is missing,
with an installation explanation. A selected profile is saved explicitly;
Kiln does not guess a new connection's active account. Use refresh after signing
in or adding a CLI profile.

Oracle API-key and session-token profiles are supported; a profile containing
`security_token_file` uses explicit `--auth=security_token` without Kiln reading
that file. Refresh an expired session with the official CLI before reconnecting.
Oracle's region can come from the selected configuration. Its namespace is
optional: the CLI resolves it when omitted. Google's ADC source is separate
from its named CLI configurations. Cloudflare requires the destination account
ID as well as the selected Wrangler profile. Wrangler resolves its own global
credential directory; if that directory changes, reselect the profile rather
than silently authenticating from another source.

Native OAuth/API-signing profiles are **not S3 interoperability keys**. The
separate S3-compatible mode accepts provider-issued interoperability credentials
or an explicitly selected AWS-format named profile. Supply that provider's
endpoint and signing region; R2 uses region `auto`. Legacy S3 connections keep
their existing saved format and credential-store entries.

## Transfers and moves

Downloads stream to a temporary file beside the destination. The destination
is replaced only after the complete transfer and a local sync succeed. Errors
and cancellation preserve the previous local file. Uploads stream instead of
loading the whole file into memory. Cancelling a job stops its request or its
owned CLI process group; it does not terminate unrelated terminals or programs.
Remote requests already accepted by a provider can complete even when a client
cancels, so refresh the listing before retrying a mutation.

Oracle uses its native rename operation. Google uses server-side rewrite, waits
for every rewrite page, and deletes only the original source generation after
the destination is complete. A failed rewrite leaves the source in place; an
interruption after copying can leave both objects.

Cloudflare's native object API has a **300 MB upload limit** (Kiln applies a
conservative 300,000,000-byte limit). It has no server-side rename operation,
so a move of a confirmed Standard-class plain byte object without customer-supplied
encryption uses an owned temporary download, uploads the
destination with an explicit Standard class, verifies the returned class, checks
the source's ETag again, then deletes the source. Unknown or non-Standard classes
and customer-supplied encryption require S3-compatible mode. The native
API does not expose atomic upload/delete preconditions: collision and source
checks have a concurrent-writer window. Metadata-bearing objects and object
names containing `.` or `..` path segments require S3-compatible mode for moves
or access. A failed or cancelled copy never triggers the source deletion;
successful copying followed by a failed check/delete can leave both names.
Use S3-compatible mode for larger R2 uploads or metadata-bearing moves.

CLI operations and HTTP requests have bounded deadlines and support cancellation.
Tokens stay transient in the worker; CLI stderr, authentication output, and cloud
error response bodies are not included in application error messages or logs.
No real provider authentication or cloud mutation is required by the fixture
tests. Successful fixtures do not prove a user's account policies or a live cloud
transfer; those require an explicitly authorized disposable bucket test.

## References

- [Google Cloud CLI configurations](https://docs.cloud.google.com/sdk/docs/configurations)
- [Google ADC token command and explicit credential-file environment](https://docs.cloud.google.com/sdk/gcloud/reference/auth/application-default/print-access-token)
- [Cloud Storage insert preconditions](https://docs.cloud.google.com/storage/docs/json_api/v1/objects/insert)
- [Cloud Storage rewrite and generation preconditions](https://docs.cloud.google.com/storage/docs/json_api/v1/objects/rewrite)
- [OCI object operations](https://docs.oracle.com/en-us/iaas/tools/oci-cli/latest/oci_cli_docs/cmdref/os/object.html)
- [OCI atomic rename destination precondition](https://docs.oracle.com/en-us/iaas/tools/oci-cli/latest/oci_cli_docs/cmdref/os/object/rename.html)
- [OCI session token profiles and required auth mode](https://docs.oracle.com/en-us/iaas/Content/API/SDKDocs/clitoken.htm)
- [OCI configuration source precedence](https://github.com/oracle/oci-python-sdk/blob/master/src/oci/config.py)
- [Wrangler profile selection](https://developers.cloudflare.com/workers/wrangler/profiles/)
- [Wrangler token command formats](https://developers.cloudflare.com/changelog/post/2025-12-18-wrangler-auth-token/)
- [Official Wrangler token-handler source](https://github.com/cloudflare/workers-sdk/blob/main/packages/wrangler/src/user/commands.ts)
- [Wrangler global credential paths](https://github.com/cloudflare/workers-sdk/blob/main/packages/workers-utils/src/global-wrangler-config-path.ts)
- [R2 native object API and upload limit](https://developers.cloudflare.com/api/resources/r2/subresources/buckets/subresources/objects/methods/upload/)
- [R2 S3 credentials and endpoint](https://developers.cloudflare.com/r2/get-started/s3/)

Implementation: `kiln-remote/src/object_profiles.rs`, `cloud_cli.rs`,
`object_oci.rs`, and `object_backend.rs`. Tests use private configuration files,
synthetic CLI executables, and loopback HTTP fixtures, never a real HOME profile.

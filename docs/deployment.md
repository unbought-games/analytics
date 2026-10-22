# Self-hosting with Docker

Deploy one analytics service against your own PostgreSQL database. Administration
and analysis use authenticated MCP or HTTP; there is no web dashboard. The service
needs no particular hosting provider, identity provider, or LLM subscription.

```text
Your websites / backend producers / MCP clients
                       |
              HTTPS reverse proxy
                       |
        Analytics container (port 8080)
               |                 |
        PostgreSQL database   Persistent export files
```

Optional read-only connectors query your PostgreSQL, Prometheus or Sentry sources.
Those services remain independently operated. This guide describes the supported
setup; [acceptance evidence](acceptance.md) records local validation, not a live
deployment or published release.

## 1. Prepare the host and database

You need Docker with Compose v2, a checkout of the revision you intend to run,
a reachable PostgreSQL database, and a domain with an HTTPS reverse proxy.
Python 3 is needed only for the included administration client. The Dockerfile
builds the Rust binary; the host does not need Rust or Node to run the service.
PostgreSQL 17 is exercised by the disposable acceptance stack.

Provision a dedicated empty database and a login that owns its schema. This same
login applies migrations and performs normal reads/writes. It needs permission to
create/alter objects in its schema, but no superuser, CREATEDB or CREATEROLE
privileges. The deployment Compose file does not create PostgreSQL for you.
Keep the database on a private network and configure transport security for your
hosting environment. Use a host reachable from inside the container; `localhost`
there means the analytics container, not the Docker host.

Run the following commands from the repository root. Keep the checkout directory
and Compose project name stable across upgrades: the named data volume belongs
to that project. If setting `COMPOSE_PROJECT_NAME`, use the same value for every
command and keep separate installations under separate project names.

## 2. Configure and start

Create an ignored `.env` beside `compose.deploy.yaml` with mode `0600`. Enter
these values using your editor or secret-management tooling:

```dotenv
DATABASE_URL=postgres://analytics:<password>@db.example:5432/analytics
ANALYTICS_PUBLIC_BASE_URL=https://analytics.example
```

Replace the synthetic hostnames and password. URL-encode reserved characters in
connection-string credentials and use your database provider's required TLS
connection options. Keep the real file out of source control and logs.
`ANALYTICS_PUBLIC_BASE_URL` must be the exact external HTTPS origin, with no path,
query or fragment. Local-only smoke tests may use `http://127.0.0.1:8080`.

```sh
docker compose -f compose.deploy.yaml up -d --build --wait
curl --fail --silent --show-error http://127.0.0.1:8080/readyz
```

Startup applies pending migrations before opening the HTTP listener. No separate
migration job or database credential is required. A failed migration prevents
startup. The container runs as UID/GID `65532:65532` and restarts unless stopped.

The Compose file publishes `127.0.0.1:8080` on the host. Point a host-based HTTPS
reverse proxy at that address. A proxy in another container needs a shared Docker
network and the service address `analytics:8080`; its own loopback cannot reach
this service. Route the whole public origin, including `/mcp`, `/v1/*` and
`/.well-known/*`, without rewriting paths.

The proxy must preserve authentication, Origin and MCP headers, support CORS
preflight and streaming responses/downloads, and avoid caching authenticated
responses. Configure connection/header timeouts and request-size limits there;
application admission does not bound incoming TCP connections or incomplete
headers. Allow enough response time for permitted downloads (the default service
stream deadline is five minutes). See [request limits](request-admission.md).

Verify the external route before configuring clients:

```sh
curl --fail --silent --show-error https://analytics.example/readyz
```

A healthy response proves database readiness, not that properties are configured
or events are being collected.

## 3. Create the first administrator

Run once after the service starts:

```sh
docker compose -f compose.deploy.yaml exec analytics analytics-server bootstrap \
  --owner "Owner" --agent "Admin Agent" \
  --output /var/lib/analytics/handoff/initial.json --expires-days 30
```

Bootstrap creates separate owner and agent principals with owner authority. The
agent token is written once to a mode-0600 file, never printed. A second bootstrap
is refused. No OIDC provider or browser setup page is required.

Copy the handoff to a new file in a protected local directory:

```sh
mkdir -p local/handoff
chmod 700 local/handoff
(umask 077; docker compose -f compose.deploy.yaml cp \
  analytics:/var/lib/analytics/handoff/initial.json local/handoff/initial.json)
chmod 600 local/handoff/initial.json
python3 examples/admin/mcp_client.py --url https://analytics.example \
  --credentials local/handoff/initial.json --tool identity
python3 examples/admin/mcp_client.py --url https://analytics.example \
  --credentials local/handoff/initial.json --tool status
```

Do not overwrite an existing local handoff. Check the returned principal and
grants. Configure other MCP clients with `https://analytics.example/mcp` and their
protected bearer-token facility. The Python client's `--url` takes the origin and
adds `/mcp` itself. A client that only supports interactive OAuth needs the optional
[human sign-in configuration](authentication.md#human-sign-in-and-remote-oauth).

The bootstrap credential expires in 30 days. Issue scoped credentials for routine
clients, then rotate/revoke credentials through
[credential administration](authentication.md#programmatic-administration).
Use `--secret-output` with the Python client when issuing or rotating a credential;
the new token must not be printed or committed. Revocation also affects exports
created by that credential; see [export access and lifecycle](exports.md).

## 4. Configure collection and verify an export

Complete these steps through your authenticated MCP client before treating the
installation as ready to collect data:

1. Create a property with its exact website origins, collection profile, retention
   periods and event schemas using `admin_execute`. Choose the collection settings
   described in [privacy controls](privacy.md); identity depth and cross-property
   linking are explicit choices.
2. Issue a public property token using `ingest_token_issue`, register the browser
   schemas, and install the [browser package](browser.md#build-and-integrate).
   The browser receives only the public token. The Docker image does not build or
   serve the browser package; build it from this checkout and integrate the output
   with your site. Do not assume an npm release has been published.
3. For authoritative backend events, issue a property-scoped credential and follow
   the [transactional producer examples](../examples/producers/README.md). Keep
   durable capture/retries in your application so an analytics outage does not
   turn committed business events into missing evidence.
4. Use an explicitly designated test property and synthetic events to verify the
   browser and backend paths. Check per-event accepted/duplicate acknowledgements
   and `ingest_status`; HTTP success alone is insufficient. Confirm disallowed
   origins and unauthorized requests are rejected.
5. Discover the property with `query_analytics_discover`, create a bounded export
   with `export_create`, poll `export_status`, then fetch `export_manifest` and
   download its authenticated dataset URL from your client machine. Verify the
   declared byte count and SHA-256. The [export guide](exports.md) describes the
   request format and completion/partial/failure states.

Optional [external sources](sources.md) use separately restricted read-only
credentials. Configure human sign-in and additional principals/grants through
[administration](authentication.md). Record the deployed Git revision, effective
configuration and successful verification date in your private operating records.

## Configuration reference

The supplied Compose file forwards only `DATABASE_URL` and
`ANALYTICS_PUBLIC_BASE_URL`. **Adding another variable to `.env` alone does not
pass it to the service.** Add it under `services.analytics.environment` in a local
Compose override and include that override in every Compose command.

| Setting | Container default / purpose |
| --- | --- |
| `DATABASE_URL` | Required by the supplied Compose file; analytics database connection |
| `ANALYTICS_PUBLIC_BASE_URL` | `http://127.0.0.1:8080`; set your HTTPS origin for remote use |
| `ANALYTICS_DATABASE_URL_FILE` | Optional protected connection file; takes precedence over `DATABASE_URL` |
| `ANALYTICS_EXPORT_DIRECTORY` | `/var/lib/analytics/exports`; persistent local export files |
| `ANALYTICS_SECRET_DIRECTORY` | `/run/analytics/secrets`; optional external connector secret files |
| `ANALYTICS_OIDC_ISSUER` | Unset; optional human sign-in provider |
| `ANALYTICS_MAX_IN_FLIGHT_REQUESTS` | `64`; concurrent application requests, range 1–4096 |
| `ANALYTICS_REQUEST_TIMEOUT_MS` | `30000`; request deadline, range 1–600000 ms |
| `ANALYTICS_STREAM_TIMEOUT_MS` | `300000`; response-stream deadline, range 1–3600000 ms |
| `RUST_LOG` | `info`; structured service logging filter |

For example, save this as ignored `local/compose.override.yaml` to enable an
OIDC issuer you configured following the authentication guide:

```yaml
services:
  analytics:
    environment:
      ANALYTICS_OIDC_ISSUER: ${ANALYTICS_OIDC_ISSUER:?set ANALYTICS_OIDC_ISSUER in .env}
```

```sh
docker compose -f compose.deploy.yaml -f local/compose.override.yaml up -d --build --wait
```

For a connection-file-only deployment, use your own local Compose file that
replaces the required `DATABASE_URL` entry with `ANALYTICS_DATABASE_URL_FILE` and
mounts that file read-only. A regular override adding the file variable does not
remove the base file's required `DATABASE_URL` interpolation. Connection and
connector secret files must be regular, not symlinks, mode `0600` or stricter and
readable by UID 65532; their parent directories must permit that user to traverse
them. A read-only mount alone does not set ownership or permissions. Keep connector
mounts under `/run/analytics/secrets` and reference filenames through source
administration. Do not copy secrets into the image.

## Persistent storage and routine operations

PostgreSQL stores configuration, grants, credential verifiers, events, identity
facts, export metadata and audit records. The `analytics_data` volume stores
export file contents and the protected bootstrap handoff. Back up both stores;
neither one alone recovers the complete installation. Connector secrets and your
local deployment configuration need their own protected backups.

Use a single service instance with the supplied Compose file. Its export files
are local to the volume; separate replicas with independent disks cannot serve
one another's files. This guide does not establish high availability, rolling
upgrade safety or a production capacity guarantee.

- Schedule `ingest_retention` for each property with `{ "property_id": "<UUID>" }`
  using a principal granted `privacy:delete` on that property. Each call removes
  up to 10,000 expired rows per table; run bounded follow-up calls until `removed`
  is zero and alert on failures or persistent backlog. Pick a frequency that
  meets your retention policy. **Event retention cleanup is not automatically
  scheduled by the service.** Export-file cleanup runs in its background worker.
- Monitor `/healthz` for liveness, `/readyz` for database readiness, `ingest_status`
  for acceptance/rejection/loss, and export outcomes for failures. Monitor host
  disk space, database growth and credential expiry using your monitoring system.
- Inspect `docker compose -f compose.deploy.yaml ps` and
  `docker compose -f compose.deploy.yaml logs --tail 100 analytics` for failures.
  Keep logs free of credentials, request bodies and analytical data. Avoid sharing
  expanded Compose configuration or environment dumps; they can contain secrets.

## Upgrades, backups and recovery

1. Record the current revision and take a consistent database/data-volume backup
   using the [backup and restore procedure](operations.md#backup-and-restore).
   Rehearse recovery in an isolated installation before relying on the backup.
2. Select the reviewed new revision, preserve `.env`, local overrides and the
   Compose project identity, then repeat `up -d --build --wait` with the same file
   arguments. Plan an interruption for this single-instance deployment.
3. Migrations run automatically; concurrent migrations are locked, and applied
   migration checksums are verified. Recheck public readiness, authenticated
   identity, ingestion and an export download after the update.
4. If startup reports `migration_failed`, inspect the database availability,
   schema privileges and revision/migration compatibility. Do not edit migration
   history to force startup. A previous binary is not guaranteed to understand
   the migrated schema: rollback may require restoring the matched pre-upgrade
   database and file backup with the previous revision.

Use `docker compose -f compose.deploy.yaml stop` for a normal stop. Container
recreation preserves the named volume; `down --volumes` deletes it and must not
be used as a routine update or restart command.

If all administrative access is lost, recover an existing principal with host
access. Replace the UUID and use a new output filename:

```sh
docker compose -f compose.deploy.yaml exec analytics analytics-server recover \
  --principal '<principal-UUID>' --revoke-all \
  --output /var/lib/analytics/handoff/recovery.json --expires-days 30
```

Recovery revokes that principal's existing credentials and OIDC bindings. Copy
the protected file using the bootstrap handoff procedure, verify `identity`, and
reconfigure affected clients. `bootstrap` cannot reset an initialized installation.

## Common setup problems

| Symptom | Check |
| --- | --- |
| Container never becomes healthy | Database hostname from inside the container, credentials, schema ownership, migration failure, or OIDC discovery failure when enabled |
| Local readiness works, public URL fails | DNS, certificate, proxy upstream/network and path routing |
| Agent gets 401/403 | Correct `/mcp` endpoint, token expiry/revocation, resource grants, preserved Authorization header; a public property token cannot authorize MCP |
| Browser `ready` rejects or no events appear | Exact allowed origin, public token, registered schemas, current policy version and visitor choice; inspect per-event rejection reasons |
| Export metadata exists but download fails | Credential/source lifecycle, expiry, proxy streaming deadlines, volume mount/ownership and available disk space |
| Optional configuration is ignored | Environment entries and mounts in the actual Compose service, with the override supplied to every command |

For test stacks and detailed validation evidence, see [operations](operations.md)
and [acceptance](acceptance.md). Use `compose.deploy.yaml` for this deployment;
`compose.yaml` and `make local-up` belong to the disposable synthetic test harness.

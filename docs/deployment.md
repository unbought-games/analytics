# Docker deployment

Run one analytics container against an existing PostgreSQL database. The same
account owns the analytics schema and handles normal reads/writes. The server
applies pending migrations before accepting requests on every start. There is no
separate migration container or second database credential.

## Start

Create a dedicated empty database and a login that owns it. The login needs to
create/alter tables in its schema; it does not need PostgreSQL superuser,
CREATEDB or CREATEROLE privileges. PostgreSQL itself must already be running.

Create an ignored `.env` file beside `compose.deploy.yaml`, with mode `0600`, and
set these values through your editor or secret-management tooling:

```dotenv
DATABASE_URL=postgres://analytics:<password>@db.example:5432/analytics
ANALYTICS_PUBLIC_BASE_URL=https://analytics.example
```

Use a database host reachable from inside the container. `localhost` there means
the analytics container, not the Docker host. For a local-only smoke test, use
`ANALYTICS_PUBLIC_BASE_URL=http://127.0.0.1:8080`.

```sh
docker compose -f compose.deploy.yaml up -d --build --wait
```

The example publishes port 8080 on host loopback. For remote use, configure your
HTTPS reverse proxy to forward the public origin to `127.0.0.1:8080`. It must
support streaming MCP responses and artifact downloads. Export files and the
bootstrap handoff persist in the `analytics_data` volume.

## Create the first admin

Once, after starting the container:

```sh
docker compose -f compose.deploy.yaml exec analytics analytics-server bootstrap \
  --owner "Owner" --agent "Admin Agent" \
  --output /var/lib/analytics/handoff/initial.json --expires-days 30
```

This creates the owner and an agent with administrative access. The generated
token is written to a mode-0600 file, never printed. Bootstrap refuses to run
again after initialization. No OIDC provider or browser setup page is required.

Copy the credential file to a protected local directory to configure your MCP
client:

```sh
mkdir -m 700 -p local/handoff
(umask 077; docker compose -f compose.deploy.yaml cp \
  analytics:/var/lib/analytics/handoff/initial.json local/handoff/initial.json)
chmod 600 local/handoff/initial.json
python3 examples/admin/mcp_client.py --url https://analytics.example \
  --credentials local/handoff/initial.json --tool identity
```

Use a new destination for the copy. Keep the file out of source control. Configure
an MCP client with the `/mcp` URL and its protected bearer-token facility, or use
the supplied Python client. Through MCP/API, create your properties, allowed
origins, collection policies, browser event schemas and public ingestion tokens.
See [browser setup](browser.md) and [administration](authentication.md). The
bootstrap credential expires after 30 days here; rotate it before expiry using
credential administration.

## Updates and configuration

Build/start the new revision using the same Compose command. Pending migrations
run automatically against the existing database; applied migrations are checked
and skipped. Concurrent starts use PostgreSQL migration locking. A failed
migration stops startup before the HTTP listener opens. Back up PostgreSQL and
the data volume before upgrading; see [backup and restore](operations.md#backup-and-restore).

`DATABASE_URL` is the simple connection option. A mounted protected connection
file remains supported through `ANALYTICS_DATABASE_URL_FILE`; if set, it takes
precedence over `DATABASE_URL`. The file must be regular, not a symlink, mode
`0600` or stricter, and readable by the container user (UID 65532 by default).

The image defaults to `/var/lib/analytics/exports` for exports and
`/run/analytics/secrets` for external connector secrets. Mount connector secrets
there if needed. Human OIDC sign-in is optional; configure it as described in
[authentication](authentication.md#human-sign-in-and-remote-oauth).

`analytics-server migrate` remains available for applying migrations explicitly,
but normal deployment does not require it. `bootstrap` also applies migrations,
so it can run before the server on an empty database. No command creates the
PostgreSQL database or its login for you.

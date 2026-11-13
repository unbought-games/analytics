# Server-owned acquisition

The existing server ingestion contract accepts navigation, signup and enrollment
observations. No browser SDK is required to capture or deliver them. Use the
application's own durable records/outbox; the collector is not part of the
business transaction.

Capture document requests in application middleware. Retain an anonymous journey
and its first eligible source in a signed first-party cookie or server-side
session. Keep queries/fragments out of URL fields and parse campaign labels
separately. Unknown acquisition stays unknown. A referrer is observed evidence,
not proof of a unique person; document requests also include bots and previews.

For authentication across origins, carry a bounded source snapshot through the
normal authorization request. An implementation may use `analytics_context`
containing base64url JSON `{at,page,route,referrer,campaign}`. Validate its time
window, URL origins and labels against the registered client/redirect. Transfer
no subjects, visitor IDs or credentials. These labels have the same evidentiary
status as a public referrer or UTM parameter; they never authorize identity links.
Persist the accepted source locally before registration. Keep invite codes out of
this snapshot. Shared landing links do not need tracking parameters.

Persist an immutable source snapshot in the transaction that creates the user.
A worker projects the completed registration once, using its original creation
time and verified subject. Include the selected source campaign on the signup
payload, with the server-created journey IDs and `context.source_occurred_at`
set to the original source time. The collector creates the touch at that time and
the acquisition in the same transaction, even when signup delivery precedes
earlier page batches. Source times are accepted only on server signup events;
future values are rejected and sources outside the effective attribution window
do not create a touch. Existing earlier eligible touches still win; later logins cannot
rewrite acquisition. Use `context.authentication_method` for the observed method.

Capture enrollment with its membership transaction, using the verified
issuer/subject mapping and the source context allowed by the current request.
Stable fact keys prevent overlap with optional browser identity bridges from
creating another signup or enrollment. A missing or disabled browser SDK must not
suppress these server facts. Exercise rollback, retry, unavailable collectors,
existing accounts, shared-device boundaries and collection preferences.

Deploy the additive `context.route` and `context.authentication_method` / `context.source_occurred_at` collector
contract before producers that emit those fields. Older collectors reject unknown
context fields. Routes use full origin plus named path; page paths retain ordinary
entity values for drill-down. Never treat server HTTP requests as unique humans
or in-app SPA transitions as additional server page requests.

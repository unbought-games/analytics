# Clean landing attribution

Base: e99bf88 plus the changes in this commit. Added optional, sanitized
`landingCampaign` for the initial browser page view. Uses the existing wire
campaign/touch, retention, identity and handoff contracts; no server migration.
Explicit URL campaigns win, prior first touch survives, and collection policy
continues to govern all observations. No address-bar/history mutation.

Validation on 2026-11-12:
- `npm ci --ignore-scripts`: pass (0.68 s).
- `npm run check`: pass.
- `npm run test --workspace ./packages/browser -- --grep 'clean landing|landing attribution|disabled collection does not'`: 3 passed (1.6 s).
- `npm run test --workspace ./packages/browser`: 33 passed (8.6 s), including existing policy, privacy, identity, delivery and cross-property browser contracts.
- `git diff --check`: pass.

One initial invocation through the root script lost `--grep` while forwarding
arguments and selected no tests. Correcting command forwarding ran the tests;
there were no test assertion failures or test repairs. Timings above are runner
reported; compilation/setup and queue wait were not separately measured.
Playwright's task-owned fixture server was cleaned up by the runner. This is
controlled browser/HTTP fixture evidence, not a deployed end-to-end signup proof.
No production access, deployment or package publication was performed.

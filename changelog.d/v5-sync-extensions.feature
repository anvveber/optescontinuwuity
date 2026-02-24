Several Simplified Sliding Sync (MSC4186) spec-compliance fixes and improvements:
- Receipts (MSC3960): were previously sent for every room regardless of the `enabled` flag or `lists`/`rooms` filter fields; now correctly scoped.
- Typing (MSC3961): filter variables were computed but never applied, so typing events were sent for all rooms; now correctly filtered.
- Account data (MSC3959): per-room data was collected for every todo room ignoring the extension's `lists`/`rooms` filters; now correctly scoped and uses per-room `since` timestamps instead of the global one.
- `required_state` on incremental syncs now skips a database round-trip entirely when no state is requested and nothing else has changed.
- `required_state` on incremental syncs now only sends events that have changed since they were last delivered, reducing response sizes.
Contributed by @0xnim

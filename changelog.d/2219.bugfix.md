Fixed federated typing indicators stopping after the first notice. Outgoing
transaction IDs are no longer a pure content hash, so identical `m.typing` EDUs
are not dropped as duplicates. Contributed by @finalish0.

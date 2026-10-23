# Bounded conversation synchronization

Approved in the maintainer conversation on 2026-10-23, including implementation and release.

Each workspace configures its recent logical conversation limit (default 20, range 1..4096). Copies across agent namespaces count once. The limit applies continuously to discovery and previously tracked hub conversations. Excluded histories remain on disk and are not synchronized or archived as part of pruning. Included conversations retain complete contents. Rank using native conversation activity, never Kiln projection write times. Resuming an old conversation makes it eligible again. Preserve echo suppression, exact message order, images, branches, read-only Antigravity sources and active-writer guards.

Repair unchanged alias retry/caching and quadratic Claude compaction reconstruction. Separate full discovery cadence from known-file polling and schedule a quiet interval after completed scans. Verify content and no-echo behavior separately from performance; do not claim this proves system-wide keyboard/mouse latency is fixed.

Ship a new immutable version with macOS notarization and signed update archives for macOS and Windows x64/ARM64. Preserve real user sessions. User authorization covers implementation, push, merge and release for this change.

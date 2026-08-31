# M40 Level-1 release-gate evidence

Environment metadata and immutable gate output for the `1.0.0-rc.1`
candidate are stored in this directory. `fuzz.txt`, `interop.txt`, and
`security-performance.txt` are complete. `soak.txt` preserves an explicitly
interim development run stopped at its 12,420-second checkpoint after
735,873,468 sessions. It is not a `PASS` result and does not satisfy the
86,400-second M40 release gate.

Every file records the exact command and source revision. The executable gate
definitions remain in `tools/`; this directory is an audit record, not a
replacement for rerunning them.

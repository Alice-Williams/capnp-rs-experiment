# M40 Level-1 release-gate evidence

Environment metadata and immutable gate output for the `1.0.0-rc.1`
candidate are stored in this directory. `fuzz.txt`, `interop.txt`, and
`security-performance.txt` are complete. `soak.txt` is added only after the
full 86,400-second run finishes; a smoke run is not release evidence.

Every file records the exact command and source revision. The executable gate
definitions remain in `tools/`; this directory is an audit record, not a
replacement for rerunning them.

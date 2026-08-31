# M48 release-commit performance evidence

These artifacts were produced in the Debian Trixie development container on
the G-drive Docker Desktop environment. Each milestone subdirectory records
its producing Git commit, toolchain, host context, source hashes, and raw
measurements.

`m29-attempt1` is intentionally retained as rejected evidence. Its 8,192-item
single-partition control took 4,003,438 ns versus 3,635,125 ns for the serial
control, exceeding the 5% noise/regression bound. An immediate independent
rerun in `m29` passed both below-threshold controls and produced qualifying
four-worker measurements. Keeping the rejected attempt makes the rerun and
the gate decision auditable.

`m30-attempt1` and `m30-attempt2` are also retained as rejected evidence. The
1,024-item control missed the 5% bound twice while the parallel sizes exceeded
3.3x. Inspection found that the benchmark used the globally indexed checked
setter inside an already range-bounded partition. Commit `8f1542c` changed the
workload to the intended partition-local hot path; `m30` was generated from
that commit and passes without relaxing the gate.

`m31-attempt1` and `m31-attempt2` retain two rejected seven-sample runs. Their
one-message configurations both used the same serial branch but differed by
more than 5%, showing that seven medians were too sensitive to host scheduling
noise. Commit `79ebaa7` raised the default to 31 odd samples and strengthened
artifact provenance; `m31` passes the unchanged 5% and 3.0x gates with that
sample count.

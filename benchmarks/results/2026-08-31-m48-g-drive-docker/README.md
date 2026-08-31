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

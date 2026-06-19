# KWin Active Mode Verification Design

## Problem

After KWin enables the virtual connector, the daemon verifies the requested
pixel mode against `Geometry` from `kscreen-doctor -o`. `Geometry` is expressed
in logical coordinates, so display scaling can make a correctly applied mode
appear smaller. For example, a 3840x2160 mode at scale 1.5 has a 2560x1440
geometry and is incorrectly rolled back as a CRTC timeout.

## Design

Keep `Geometry` exclusively for layout snapshots and positioning. Parse the
current mode from the `Modes:` entry marked with `*`, associating it with the
validated connector block that contains it. The final connect verification
must require:

- the requested connector exists and is enabled;
- its active mode width and height equal the requested pixel dimensions;
- its active refresh differs from the requested integer refresh by no more
  than 0.5 Hz, allowing representations such as 59.94 for 60 Hz.

The parser is internal and does not change `OutputInfo`, the recovery journal,
or the IPC protocol. Malformed or missing active-mode data fails closed.

## Errors

Replace the shared timeout error with distinct internal strategy errors:

- output detection timeout when the connector never appears before the
  configured deadline;
- mode verification failure when KWin responds but the connector is disabled,
  has no parseable active mode, or applies a different mode.

## Tests

Add regression coverage proving that a scaled output matches its physical
active mode rather than its logical geometry. Cover 59.94/60 Hz tolerance,
wrong dimensions, wrong refresh, preferred (`!`) versus active (`*`) markers,
and malformed mode tokens. Run formatting, Clippy, daemon tests, and the full
workspace test suite. Hardware acceptance is a successful connect and
disconnect on the existing scaled KWin session.

# GTFS-Realtime parity

Tools for comparing GTFS Guru against the pinned canonical validator,
`MobilityData/gtfs-realtime-validator` at commit `7041fa3f`.

The pins live in `crates/gtfs_validator_rt/spec_baseline.json`: the Java commit,
the built JAR's SHA-256, and the GTFS-Realtime schema revision. Every artefact
here is meaningful only against those pins.

## `decoder_java_check.java`

Replays the decoder fixtures through the canonical bindings
(`gtfs-realtime-bindings:0.0.4`, bundled in the JAR) and prints what Java does
with each one, so the compatibility behavior pinned by
`crates/gtfs_validator_rt/tests/decoder.rs` can be compared against it. Raw
`prost` differences explain why the compatibility boundary exists; they are not
accepted default-profile deltas.

```bash
cargo test -p gtfs-guru-rt --test decoder -- --ignored dump_fixtures
java -cp "$GTFS_RT_VALIDATOR_JAR" scripts/rt_parity/decoder_java_check.java \
     target/rt-decoder-fixtures
```

Needs a JDK 11+ for the single-file source launcher. Not part of CI: the JAR is
not reproducible from its commit and is not committed.

## `expected_deltas.json`

The ledger of differences from the canonical validator that are **accepted
rather than fixed**.

Two rules from GTF-11 govern it, and neither is negotiable:

* **Never approve an implementation mistake as an expected delta.** A delta
  records a difference that is intended and explained. A bug is fixed.
* **A stale approval must fail parity and be removed.** An entry that no longer
  matches an observed difference is standing permission for something nobody has
  looked at since.

### Lifecycle

1. **Phase 0** — define the policy and record format. Done; this file and
   `check_expected_deltas.py` are it.
2. **Phases 1-3** — add focused differential evidence while rules are
   implemented, one rule at a time. Do not wait for a complete corpus.
3. **Phase 4** — consolidate and approve the corpus-level differences observed
   against the recorded snapshots.
4. **Later phases** — add their own measured deltas.

### Status

`proposed` is a difference that has been *demonstrated* but not yet *decided*:
both sides are recorded, and `blockedOn` names the question that settles it. It
is not permission for anything.

`approved` is a difference that has been decided and may stand. An approved
entry may not carry `blockedOn`.

The ledger is currently empty. The two Phase 0 decoder proposals were removed
after GTF-11 selected Java compatibility and the loading boundary matched Java's
required-field and unknown-enum behavior, including known-then-unknown enum
ordering.

### Fields

Every entry records what GTF-11 requires of an approved delta.

| Field | Meaning |
| :--- | :--- |
| `id` | Stable identifier for the difference. |
| `status` | `proposed` or `approved`. |
| `blockedOn` | What settles a proposed delta. Required on `proposed`, forbidden on `approved`. |
| `canonicalRuleId` | Canonical E/W identifier, or `null` for a divergence that belongs to no rule. Must be present. |
| `noticeCode` | GTFS Guru notice code, or `null` before the rule matrix is frozen. Must be present. |
| `fixture` | The fixture or corpus entry that demonstrates it. |
| `javaResult` / `rustResult` | What each side actually does. |
| `expected` | Expected counts or normalised fingerprints, per side. |
| `reason` | Why the difference is intended, not a defect. |
| `specReference` | Where the specification supports the reading taken. |
| `removalCondition` | What would make this entry stale. |
| `recordedAt` | When the difference was observed. |
| `javaCommit`, `javaJarSha256`, `rtSchemaCommit`, `bindingsSchemaCommit`, `bindingsSchemaSha256` | Inherited from the ledger's `baseline` block unless an entry overrides them, so every delta resolves to one oracle binary and both schemas without repeating them by hand. |

### Checking it

```bash
python3 scripts/rt_parity/check_expected_deltas.py
```

Offline. It verifies the record format, rejects duplicate ids and inconsistent
statuses, and — the part that matters most — fails when the ledger's baseline
does not match the pins in `crates/gtfs_validator_rt/spec_baseline.json`. An
approval describes a difference against one binary and both its public-model
and loading-boundary schemas; if any pin moves, every approval in the file is
about something else until it is re-observed.

`check_expected_deltas_test.py` covers it offline by injecting one fault at a
time.

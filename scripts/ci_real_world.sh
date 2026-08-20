#!/usr/bin/env bash
# Real-world corpus parity, the way CI runs it.
#
#   MODE=self  gtfs.guru alone against the committed baseline (fast, per commit)
#   MODE=full  adds the pinned MobilityData/gtfs-validator baseline (tags, cron)
#
# Set GTFS_VALIDATOR_BIN to a release binary; without it the harness refuses to
# run rather than building the multi-gigabyte debug tree.
set -euo pipefail

mode="${MODE:-self}"
out_dir="${REAL_WORLD_ACTUAL_DIR:-real_world_actual}"
results="${out_dir}/results.json"
summary_file="${REAL_WORLD_SUMMARY:-${out_dir}/summary.md}"
threads="${REAL_WORLD_THREADS:-1}"

case "$mode" in
  self) tools="guru" ;;
  full) tools="guru,java" ;;
  *) echo "MODE must be 'self' or 'full', got '${mode}'" >&2; exit 1 ;;
esac

echo "::group::Fetch pinned corpus"
python3 scripts/real_world_corpus.py fetch
echo "::endgroup::"

if [ "$mode" = "full" ]; then
  echo "::group::Java baseline"
  python3 scripts/real_world_parity.py jar --check-latest
  echo "::endgroup::"
fi

echo "::group::Validate corpus (${tools})"
python3 scripts/real_world_parity.py run \
  --tools "$tools" \
  --threads "$threads" \
  --out-dir "$out_dir" \
  --results "$results"
echo "::endgroup::"

# The gate exits 2 on a parity regression, so capture it instead of letting
# set -e abort before the summary is published.
status=0
python3 scripts/real_world_parity.py gate "$results" \
  --summary "$summary_file" \
  --json "${out_dir}/gate.json" \
  ${STRICT_PERF:+--strict-perf} || status=$?

if [ -n "${GITHUB_STEP_SUMMARY:-}" ] && [ -f "$summary_file" ]; then
  cat "$summary_file" >> "$GITHUB_STEP_SUMMARY"
fi

exit "$status"

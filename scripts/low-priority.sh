# Sourced first by gate.sh, quick-gate.sh and ff-merge.sh (T-281): with CP_LOW_PRIORITY=1 the
# script runs again under macOS background QoS (`taskpolicy -b`), and so does every cargo and
# test process it starts, so a build beside the game leaves the game its cores (16 busy
# processes at the default QoS took it from 120 to 76 fps; at background QoS it kept 117).
# Builds then use the efficiency cores and take about 2-3x longer. Default off; the Tech Lead
# turns it on for the agents while the user plays. Elsewhere (no taskpolicy) it does nothing.
if [ "${CP_LOW_PRIORITY:-0}" = 1 ] && [ -z "${CP_LOW_PRIORITY_ACTIVE:-}" ] &&
  command -v taskpolicy >/dev/null 2>&1; then
  export CP_LOW_PRIORITY_ACTIVE=1
  exec taskpolicy -b "$0" "$@"
fi

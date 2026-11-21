#!/bin/bash
# Memory + idle CPU of the off-screen test instance (see shot.sh for the env options).
# usage: tools/measure.sh [seconds-to-settle] [cpu-sample-seconds]
SETTLE=${1:-10}; SAMPLE=${2:-10}
OUT=${XDG_RUNTIME_DIR:-/tmp}/crate-measure.png
( sleep "$SETTLE"
  pid=$(pgrep -x crate-native | head -1)
  [ -z "$pid" ] && { echo "not running"; exit; }
  t0=$(awk '{print $14+$15}' /proc/$pid/stat); sleep "$SAMPLE"; t1=$(awk '{print $14+$15}' /proc/$pid/stat)
  hz=$(getconf CLK_TCK)
  awk -v a=$t0 -v b=$t1 -v h=$hz -v s=$SAMPLE 'BEGIN{printf "cpu: %.2f%% of one core over %ss\n",(b-a)*100/h/s,s}'
  grep -E "^(Rss|Pss|Pss_Anon|Pss_File)" /proc/$pid/smaps_rollup | awk '{printf "%s %.1f MB\n",$1,$2/1024}'
  echo "threads: $(ls /proc/$pid/task | wc -l)"
) &
WAIT=$((SETTLE+SAMPLE+1)) "$(dirname "$0")/shot.sh" 1320 840 "$OUT" $((SETTLE+SAMPLE+1)) >/dev/null
wait

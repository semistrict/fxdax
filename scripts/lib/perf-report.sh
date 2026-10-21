# shellcheck shell=bash
# Copyright 2026 The fxdax Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Sourced: perf_report DATA PREFIX says where the samples in DATA, a perf.data
# recorded with call graphs, went. PREFIX.txt has them by thread, by thread and
# symbol, and the call graphs of the symbols over 1%, callers first; and
# PREFIX-THREAD.txt, for each thread but the idle one with 0.5% of the samples
# or more, its own call graphs, callers first, as shares of its samples: what
# each request costs it, even where the host idles mostly. What perf complains
# of goes to PREFIX-report.log.

perf_report() {
    local data=$1 prefix=$2 comm name
    {
        echo "# By thread"
        perf report -i "$data" --stdio --no-children -g none --sort comm --percent-limit 0.05
        echo "# By thread and symbol"
        perf report -i "$data" --stdio --no-children -g none --sort comm,dso,sym \
            --percent-limit 0.2
        echo "# Call graphs of the symbols over 1%, callers first"
        perf report -i "$data" --stdio --children -g caller,0.5,callee --sort sym \
            --percent-limit 1
    } > "$prefix.txt" 2> "$prefix-report.log"
    perf report -i "$data" --stdio --no-children -g none --sort comm --percent-limit 0.5 \
        2>> "$prefix-report.log" |
        sed -n 's/^ *[0-9.]*%  *\(.*[^ ]\) *$/\1/p' | while read -r comm; do
        [[ $comm == swapper ]] && continue
        name=$(echo "$comm" | tr -c 'A-Za-z0-9\n' -)
        perf report -i "$data" --stdio --comm "$comm" --percentage relative --children \
            -g caller,2,callee,function,percent --sort sym --percent-limit 3 \
            > "$prefix-$name.txt" 2>> "$prefix-report.log"
    done
}

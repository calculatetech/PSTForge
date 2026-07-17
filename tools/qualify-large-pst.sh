#!/usr/bin/env bash
set -Eeuo pipefail

readonly MAX_LOG_BYTES=$((4 * 1024 * 1024))
readonly SOURCE=${SOURCE:?set SOURCE to the external PST path}
readonly JOB=${JOB:?set JOB to a new qualification job directory}
readonly BINARY=${BINARY:-target/release/pstforge}
readonly RESUME=${RESUME:-false}
readonly EVIDENCE_ROOT=${EVIDENCE_ROOT:-"$HOME/.local/share/pstforge/acceptance"}
readonly FIRST_PART_DEADLINE_SECONDS=${FIRST_PART_DEADLINE_SECONDS:-600}
readonly TOTAL_DEADLINE_SECONDS=${TOTAL_DEADLINE_SECONDS:-1200}
readonly TERMINATION_GRACE_SECONDS=${TERMINATION_GRACE_SECONDS:-30}
readonly MAX_JOB_MULTIPLIER=${MAX_JOB_MULTIPLIER:-3}
fresh_job=false

if [[ $SOURCE != /* || $JOB != /* ]]; then
    printf 'SOURCE and JOB must be absolute paths\n' >&2
    exit 2
fi
if [[ -L $SOURCE || ! -f $SOURCE ]]; then
    printf 'SOURCE must be a regular, non-symlink file: %s\n' "$SOURCE" >&2
    exit 2
fi
if [[ $RESUME == true ]]; then
    if [[ -L $JOB || ! -d $JOB ]]; then
        printf 'resumed JOB must be an existing, non-symlink directory: %s\n' "$JOB" >&2
        exit 2
    fi
elif [[ -e $JOB || -L $JOB ]]; then
    printf 'fresh JOB must not already exist: %s\n' "$JOB" >&2
    exit 2
elif [[ $RESUME != false ]]; then
    printf 'RESUME must be true or false\n' >&2
    exit 2
else
    fresh_job=true
fi
if [[ ! -x $BINARY ]]; then
    printf 'release binary is missing or not executable: %s\n' "$BINARY" >&2
    exit 2
fi

source_device=$(stat -Lc '%d' -- "$SOURCE")
source_inode=$(stat -Lc '%i' -- "$SOURCE")
source_size=$(stat -Lc '%s' -- "$SOURCE")
source_mtime=$(stat -Lc '%Y' -- "$SOURCE")
max_job_bytes=$((source_size * MAX_JOB_MULTIPLIER))
run_id=$(date -u +'%Y%m%dT%H%M%SZ')
evidence="$EVIDENCE_ROOT/large-qualification-$run_id"
mkdir -p -- "$evidence"
chmod 0700 "$evidence"

command_pid=
command_group=
failure_reason=

remove_failed_job() {
    [[ $fresh_job == true ]] || return 0
    [[ -n ${JOB:-} && $JOB == /* && $JOB != / && -d $JOB && ! -L $JOB ]] || return 0
    local resolved parent
    resolved=$(realpath -e -- "$JOB") || return 1
    parent=$(realpath -e -- "$(dirname -- "$JOB")") || return 1
    [[ $resolved == "$parent/"* && $resolved != "$parent" ]] || return 1
    find "$resolved" -xdev -depth -delete
}

stop_group() {
    [[ -n $command_group ]] || return 0
    kill -TERM -- "-$command_group" 2>/dev/null || true
    local deadline=$((SECONDS + TERMINATION_GRACE_SECONDS))
    while kill -0 "$command_pid" 2>/dev/null && ((SECONDS < deadline)); do
        sleep 1
    done
    if kill -0 "$command_pid" 2>/dev/null; then
        kill -KILL -- "-$command_group" 2>/dev/null || true
    fi
}

on_signal() {
    failure_reason=operator-signal
    stop_group
}
trap on_signal INT TERM HUP

printf 'source_device=%s\nsource_inode=%s\nsource_size=%s\nsource_mtime=%s\n' \
    "$source_device" "$source_inode" "$source_size" "$source_mtime" \
    >"$evidence/source-identity.txt"

started=$SECONDS
split_arguments=(
    split "$SOURCE"
    --output "$JOB"
    --max-pst-size 4GiB
    --json
    --color never
)
if [[ $RESUME == true ]]; then
    split_arguments+=(--resume)
fi
setsid /usr/bin/time -v -o "$evidence/resource.txt" \
    "$BINARY" "${split_arguments[@]}" \
    >"$evidence/result.json" \
    2> >(tee /dev/stderr | tail -c "$MAX_LOG_BYTES" >"$evidence/progress-tail.log") &
command_pid=$!
command_group=$command_pid

while kill -0 "$command_pid" 2>/dev/null; do
    elapsed=$((SECONDS - started))
    if [[ ! -f $JOB/parts/part-0001.pst ]] \
        && ((elapsed > FIRST_PART_DEADLINE_SECONDS)); then
        failure_reason=first-part-deadline
        stop_group
        break
    fi
    if ((elapsed > TOTAL_DEADLINE_SECONDS)); then
        failure_reason=total-deadline
        stop_group
        break
    fi
    if [[ -d $JOB ]]; then
        job_bytes=$(du -sx --block-size=1 -- "$JOB" | cut -f1)
        if ((job_bytes > max_job_bytes)); then
            failure_reason=job-size-limit
            stop_group
            break
        fi
    fi
    sleep 2
done

set +e
wait "$command_pid"
status=$?
set -e

if [[ -n $failure_reason ]]; then
    printf '%s\n' "$failure_reason" >"$evidence/failure.txt"
fi
if ((status == 0 || status == 1)) && [[ -f $JOB/parts/part-0001.pst ]]; then
    final_device=$(stat -Lc '%d' -- "$SOURCE")
    final_inode=$(stat -Lc '%i' -- "$SOURCE")
    final_size=$(stat -Lc '%s' -- "$SOURCE")
    final_mtime=$(stat -Lc '%Y' -- "$SOURCE")
    if [[ "$source_device:$source_inode:$source_size:$source_mtime" \
        != "$final_device:$final_inode:$final_size:$final_mtime" ]]; then
        printf 'source identity changed during qualification\n' >&2
        printf 'source-identity-changed\n' >"$evidence/failure.txt"
        remove_failed_job
        exit 1
    fi
    printf 'qualification retained at %s\n' "$JOB"
    printf 'bounded evidence retained at %s\n' "$evidence"
    exit "$status"
fi

remove_failed_job
if [[ $fresh_job == true ]]; then
    printf 'qualification failed with status %s; fresh failed job removed\n' "$status" >&2
else
    printf 'qualification failed with status %s; resumed job retained at %s\n' \
        "$status" "$JOB" >&2
fi
printf 'bounded evidence retained at %s\n' "$evidence" >&2
exit "$status"

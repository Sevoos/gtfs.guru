#!/usr/bin/env bash
# ============================================================================
# Replace the running gtfs-guru-web container with a freshly loaded image.
#
# Runs ON THE SERVER, fed over ssh by .github/workflows/deploy-web.yml:
#
#   ssh host 'bash -s -- <image>:<tag> <commit>' < deploy/swap-web-container.sh
#
# Production does not run from docker-compose.yml: Caddy on the host proxies
# gtfs.guru to a container that was started with plain `docker run`. So instead
# of hard-coding ports, env and volumes here (and silently overwriting whatever
# was tuned on the box), the script reads them off the container it replaces
# and starts the new one with the same settings. If the new container is not
# healthy within the deadline, the old image is put back.
# ============================================================================
set -euo pipefail

IMAGE="${1:?usage: swap-web-container.sh <image:tag> <commit>}"
COMMIT="${2:?usage: swap-web-container.sh <image:tag> <commit>}"
NAME="${GTFS_WEB_CONTAINER:-gtfs-validator}"
HEALTH_DEADLINE="${GTFS_WEB_HEALTH_DEADLINE:-90}"

log() { printf '[swap] %s\n' "$*"; }

docker image inspect "$IMAGE" >/dev/null 2>&1 || {
    log "image $IMAGE is not loaded on this host"
    exit 1
}

if ! docker container inspect "$NAME" >/dev/null 2>&1; then
    log "no container named $NAME to replace; refusing to guess its ports and env"
    exit 1
fi

# --- Read the live container's settings ------------------------------------
previous_image=$(docker inspect "$NAME" --format '{{.Config.Image}}')
restart_policy=$(docker inspect "$NAME" --format '{{.HostConfig.RestartPolicy.Name}}')
memory_limit=$(docker inspect "$NAME" --format '{{.HostConfig.Memory}}')

run_args=(--detach --name "$NAME" --restart "${restart_policy:-always}")
if [ "${memory_limit:-0}" != "0" ]; then
    run_args+=(--memory "$memory_limit")
fi

# Ports: "hostIp:hostPort->containerPort/proto" per published binding.
while IFS= read -r binding; do
    [ -n "$binding" ] || continue
    run_args+=(--publish "$binding")
done < <(docker inspect "$NAME" --format \
    '{{range $port, $bindings := .HostConfig.PortBindings}}{{range $bindings}}{{if .HostIp}}{{.HostIp}}:{{end}}{{.HostPort}}:{{$port}}{{"\n"}}{{end}}{{end}}')

# Named volumes and bind mounts.
while IFS= read -r mount; do
    [ -n "$mount" ] || continue
    run_args+=(--volume "$mount")
done < <(docker inspect "$NAME" --format \
    '{{range .Mounts}}{{if eq .Type "volume"}}{{.Name}}{{else}}{{.Source}}{{end}}:{{.Destination}}{{if not .RW}}:ro{{end}}{{"\n"}}{{end}}')

# Environment, minus PATH (the image sets its own) and minus the build commit
# (the new image carries its own value).
while IFS= read -r kv; do
    [ -n "$kv" ] || continue
    case "$kv" in
        PATH=*|GTFS_GURU_BUILD_COMMIT=*) continue ;;
    esac
    run_args+=(--env "$kv")
done < <(docker inspect "$NAME" --format '{{range .Config.Env}}{{.}}{{"\n"}}{{end}}')

# --- Swap ------------------------------------------------------------------
health_url=""
for binding in "${run_args[@]}"; do
    case "$binding" in
        *:3000/tcp|*:3000)
            host_part="${binding%:3000*}"
            health_url="http://127.0.0.1:${host_part##*:}"
            ;;
    esac
done
if [ -z "$health_url" ]; then
    log "could not find the published port for 3000/tcp; aborting before touching $NAME"
    exit 1
fi

wait_healthy() {
    local expected="$1" deadline="$2" reported
    for _ in $(seq 1 "$deadline"); do
        if reported=$(curl -sf --max-time 2 "$health_url/version" 2>/dev/null); then
            case "$reported" in
                *"\"commit\":\"$expected\""*) return 0 ;;
            esac
        fi
        sleep 1
    done
    return 1
}

log "replacing $NAME ($previous_image) with $IMAGE (commit $COMMIT)"
docker rename "$NAME" "$NAME-previous"
docker stop --time 20 "$NAME-previous" >/dev/null

if docker run "${run_args[@]}" "$IMAGE" >/dev/null && wait_healthy "$COMMIT" "$HEALTH_DEADLINE"; then
    docker rm "$NAME-previous" >/dev/null
    log "healthy: $(curl -sf "$health_url/version")"
else
    log "new container did not report commit $COMMIT within ${HEALTH_DEADLINE}s; rolling back"
    docker logs --tail 50 "$NAME" 2>&1 | sed 's/^/[new] /' || true
    docker rm -f "$NAME" >/dev/null 2>&1 || true
    docker rename "$NAME-previous" "$NAME"
    docker start "$NAME" >/dev/null
    exit 1
fi

# Keep the previous image for a manual rollback; drop anything older.
docker tag "$IMAGE" "${IMAGE%%:*}:latest"
docker images "${IMAGE%%:*}" --format '{{.Repository}}:{{.Tag}}' \
    | grep -vE ":(latest|${IMAGE##*:}|${previous_image##*:})$" \
    | xargs -r docker rmi >/dev/null 2>&1 || true
log "done"

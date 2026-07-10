#!/usr/bin/env bash
# Print the Client Portal Gateway base URL. Resolves the container's bridge IP
# because the published localhost:5000 is not routable from inside a devcontainer
# / docker-in-docker; falls back to localhost when the container is not found.
set -euo pipefail

ip=$(docker inspect oath-cpapi-gw -f '{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}' 2>/dev/null || true)
printf 'https://%s:5000/v1/api\n' "${ip:-localhost}"

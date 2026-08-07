#!/bin/bash
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DOCKERFILE="$ROOT/Dockerfile.codebuddy-p0"
LICENSE="$ROOT/THIRD_PARTY_LICENSES/CodeBuddy-Code-MIT.txt"

[[ -f "$DOCKERFILE" ]]
[[ -f "$LICENSE" ]]
grep -Fq '@tencent-ai/codebuddy-code@2.133.0' "$DOCKERFILE"
grep -Fq 'FROM node:20-bookworm-slim' "$DOCKERFILE"
grep -Fq 'USER freemodel' "$DOCKERFILE"
grep -Fq 'COPY THIRD_PARTY_LICENSES/CodeBuddy-Code-MIT.txt' "$DOCKERFILE"
grep -Fq 'Permission is hereby granted' "$LICENSE"
! grep -Eq 'fe_[A-Za-z0-9_-]{12,}|CODEBUDDY_(AUTH_TOKEN|API_KEY)=' "$DOCKERFILE" "$LICENSE"

COMPOSE="$ROOT/docker-compose.codebuddy-p0.yml"
[[ -f "$COMPOSE" ]]
grep -Fq '127.0.0.1:40589:40589' "$COMPOSE"
grep -Fq '127.0.0.1:44741:44741' "$COMPOSE"
grep -Fq 'codebuddy-p0-data:/data' "$COMPOSE"
! grep -Fq 'network_mode: host' "$COMPOSE"
! grep -Eq 'FREEMODEL_API_KEY:|CODEBUDDY_AUTH_TOKEN:|CODEBUDDY_API_KEY:' "$COMPOSE"

WORKFLOW="$ROOT/.github/workflows/docker-codebuddy-p0.yml"
[[ -f "$WORKFLOW" ]]
grep -Fq 'Dockerfile.codebuddy-p0' "$WORKFLOW"
grep -Fq 'codebuddy-p0' "$WORKFLOW"
! grep -Eq 'value=(beta|latest)' "$WORKFLOW"
! grep -Eq 'FREEMODEL_API_KEY|CODEBUDDY_AUTH_TOKEN|CODEBUDDY_API_KEY' "$WORKFLOW"

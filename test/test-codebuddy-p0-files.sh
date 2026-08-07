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

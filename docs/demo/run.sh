#!/usr/bin/env bash
# Scripted re-creation of a real env-wizard run, used to record docs/demo/demo.gif
# with VHS (see docs/demo/demo.tape). Not a live AI call — kept deterministic so the
# recording matches the transcript already documented in the README.
set -euo pipefail

echo "env-wizard"
echo 'At each prompt, type:'
echo '  ┃  Enter   ┃  accept the suggested default'
echo '  ┃    ?     ┃  ask the AI for a hint'
echo '  ┃   ? …    ┃  ask the AI a specific question about this variable'
echo '  ┃ (nothing)┃  leave this variable empty'
echo '  ┃    q     ┃  quit without saving'
echo 'Change the AI provider anytime with `env-wizard config`.'
echo
echo '  # Secret used to sign session cookies (32+ chars)'
printf '? SECRET_KEY › '
read -r _hint_request

echo
echo '💡 Hint'
echo 'SECRET_KEY'
echo 'This signs your session cookies. Generate one with:'
echo
echo '    openssl rand -hex 32'
echo
echo ' • Must be at least 32 characters'
echo ' • Keep it secret — put it in .env, never commit it'
echo
printf '? SECRET_KEY › '
read -r _real_value

echo '✔ SECRET_KEY · 9f2c8a…'
echo '✓ Wrote .env'

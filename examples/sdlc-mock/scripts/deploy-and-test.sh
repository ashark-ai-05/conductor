#!/bin/sh
# Deploys the service locally and checks a ticket's acceptance criteria against it.
#
#   scripts/deploy-and-test.sh tickets/BUG-101.md
#
# An acceptance criterion the script can check is a line in the ticket of the form
#   - AC2: `GET /accounts/ACC-2/balance` returns 200 with `"balance":"0.00"`.
# (the `with …` part is optional). Other bullets are for people. Conductor runs this as a
# check with the run's ticket; its output and logs/app.log become the ticket's evidence.
set -u
ticket=${1:?usage: scripts/deploy-and-test.sh <ticket.md>}
cd "$(dirname "$0")/.."
[ -f "$ticket" ] || { echo "no ticket at $ticket"; exit 1; }
rm -rf logs && mkdir -p logs
grep '^- AC[0-9]*: `GET ' "$ticket" > logs/acceptance.txt
[ -s logs/acceptance.txt ] || { echo "no checkable acceptance criteria (- ACn: \`GET …\` returns N) in $ticket"; exit 1; }
mvn -q -B -o -DskipTests package || { echo "build failed"; exit 1; }
java -jar target/accounts-0.1.0.jar > logs/stdout.log 2>&1 &
PID=$!
trap 'kill $PID 2>/dev/null; wait $PID 2>/dev/null' EXIT
for _ in $(seq 1 90); do
  curl -sf localhost:18080/accounts/ACC-1/balance >/dev/null 2>&1 && break
  kill -0 $PID 2>/dev/null || { echo "the service died on startup:"; tail -20 logs/stdout.log; exit 1; }
  sleep 1
done
echo "deployed: accounts 0.1.0 on :18080 (pid $PID), checking $ticket"
fail=0
while read -r line; do
  name=$(printf '%s' "$line" | sed -n 's/^- \(AC[0-9]*\):.*/\1/p')
  path=$(printf '%s' "$line" | sed -n 's/.*`GET \([^`]*\)`.*/\1/p')
  code=$(printf '%s' "$line" | sed -n 's/.*returns \([0-9][0-9][0-9]\).*/\1/p')
  want=$(printf '%s' "$line" | sed -n 's/.*returns [0-9]* with `\([^`]*\)`.*/\1/p')
  got=$(curl -s -w ' HTTP %{http_code}' "http://localhost:18080$path")
  ok=0
  case "$got" in *"HTTP $code"*) ok=1 ;; esac
  if [ $ok = 1 ] && [ -n "$want" ]; then
    case "$got" in *"$want"*) ;; *) ok=0 ;; esac
  fi
  if [ $ok = 1 ]; then
    echo "PASS $name: GET $path -> $got"
  else
    echo "FAIL $name: GET $path -> $got (expected $code${want:+ with $want})"
    fail=1
  fi
done < logs/acceptance.txt
exit $fail

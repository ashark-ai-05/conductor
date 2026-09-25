#!/bin/sh
# Deploys the service locally and checks BUG-101's acceptance criteria against it.
# Conductor runs this as a check; its output and logs/app.log become the ticket's evidence.
set -u
cd "$(dirname "$0")/.."
mvn -q -B -o -DskipTests package || { echo "build failed"; exit 1; }
rm -rf logs && mkdir -p logs
java -jar target/accounts-0.1.0.jar > logs/stdout.log 2>&1 &
PID=$!
trap 'kill $PID 2>/dev/null; wait $PID 2>/dev/null' EXIT
for _ in $(seq 1 90); do
  curl -sf localhost:18080/accounts/ACC-1/balance >/dev/null 2>&1 && break
  kill -0 $PID 2>/dev/null || { echo "the service died on startup:"; tail -20 logs/stdout.log; exit 1; }
  sleep 1
done
echo "deployed: accounts 0.1.0 on :18080 (pid $PID)"
fail=0
check() {
  got=$(curl -s -w ' HTTP %{http_code}' "http://localhost:18080$2")
  case "$got" in
    *"$3"*) echo "PASS $1: $got" ;;
    *) echo "FAIL $1: $got (expected $3)"; fail=1 ;;
  esac
}
check "AC1 known account"          /accounts/ACC-1/balance '"balance":"150.00"'
check "AC2 account with no postings" /accounts/ACC-2/balance '"balance":"0.00"'
check "AC3 unknown account"        /accounts/NOPE/balance  'HTTP 404'
exit $fail

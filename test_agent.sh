#!/usr/bin/env bash
set -euo pipefail

API_KEY="mcp_ixUKhIRUste0fdNvC2_77sY5-DLIKByV9MpXVJjSGSs"
BASE="http://localhost:3000/api/v1"
AGENT_NAME="test-long-runner-$(date +%s)"
POLL_INTERVAL=10

echo "=== MCPolly Test Agent ==="
echo "Registering agent: $AGENT_NAME"

REGISTER=$(curl -s -X POST "$BASE/agents/register" \
  -H "Authorization: Bearer $API_KEY" \
  -H "Content-Type: application/json" \
  -d "{\"name\": \"$AGENT_NAME\", \"description\": \"Long-running test agent for stop signal testing\"}")

echo "Register response: $REGISTER"
AGENT_ID=$(echo "$REGISTER" | python3 -c "import sys,json; print(json.load(sys.stdin)['id'])")
echo "Agent ID: $AGENT_ID"
echo "Posting status every ${POLL_INTERVAL}s. Stop me from the MCPolly dashboard!"
echo ""

post_status() {
  local state="$1"
  local message="$2"
  curl -s -X PUT "$BASE/agents/$AGENT_ID/status" \
    -H "Authorization: Bearer $API_KEY" \
    -H "Content-Type: application/json" \
    -d "{\"state\": \"$state\", \"message\": \"$message\"}"
}

RESP=$(post_status "starting" "Agent initializing...")
echo "[$(date +%H:%M:%S)] starting — $RESP"

sleep 2

ITERATION=0
while true; do
  ITERATION=$((ITERATION + 1))
  RESP=$(post_status "running" "Iteration $ITERATION — doing work...")
  echo "[$(date +%H:%M:%S)] running (iter $ITERATION) — $RESP"

  STOP=$(echo "$RESP" | python3 -c "import sys,json; print(json.load(sys.stdin).get('stop_requested', False))" 2>/dev/null || echo "False")

  if [ "$STOP" = "True" ] || [ "$STOP" = "true" ]; then
    echo ""
    echo ">>> STOP SIGNAL DETECTED! Gracefully shutting down..."
    sleep 1
    RESP=$(post_status "stopped" "Stopped by operator — acknowledged stop signal after iteration $ITERATION")
    echo "[$(date +%H:%M:%S)] stopped — $RESP"
    echo "=== Agent exited gracefully ==="
    exit 0
  fi

  sleep "$POLL_INTERVAL"
done

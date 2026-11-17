#!/usr/bin/env bash
# End-to-end smoke test for the LLM Wiki web server.
#
#   BASE=http://127.0.0.1:7860 DATA_DIR=/tmp/llmwiki-smoke ./server/smoke-test.sh <md file> <pdf file>
#
# Expects a *fresh* server (no account yet) running with the same DATA_DIR.
set -u
BASE=${BASE:-http://127.0.0.1:7860}
DATA=${DATA_DIR:?set DATA_DIR to the server data dir}
MD=${1:?markdown file}
PDF=${2:?pdf file}
JAR=$(mktemp)
H=(-H "X-LLM-Wiki: 1" -H "Content-Type: application/json")
pass=0; fail=0
ok()   { echo "  PASS  $1"; pass=$((pass+1)); }
bad()  { echo "  FAIL  $1"; echo "        -> $2" | head -c 600; echo; fail=$((fail+1)); }
check(){ # name, haystack, needle
  if grep -q -- "$3" <<<"$2"; then ok "$1"; else bad "$1" "$2"; fi
}
inv() { curl -s -b "$JAR" "${H[@]}" -X POST "$BASE/api/invoke/$1" -d "$2"; }

echo "== auth"
check "status before setup" "$(curl -s $BASE/api/auth/status)" '"configured":false'
check "page redirects to /login" "$(curl -s -o /dev/null -w '%{http_code} %{redirect_url}' $BASE/)" "30[23] .*/login"
check "invoke without session -> 401" "$(curl -s -o /dev/null -w '%{http_code}' -X POST "${H[@]}" $BASE/api/invoke/file_exists -d '{}')" "401"
check "setup without CSRF header refused" "$(curl -s -o /dev/null -w '%{http_code}' -H 'Content-Type: application/json' -X POST $BASE/api/auth/setup -d '{"password":"correct-horse"}')" "403"
check "setup short password refused" "$(curl -s "${H[@]}" -X POST $BASE/api/auth/setup -d '{"password":"short"}')" "at least 8"
check "setup" "$(curl -s -c "$JAR" "${H[@]}" -X POST $BASE/api/auth/setup -d '{"password":"correct-horse"}')" '"ok":true'
check "second setup refused" "$(curl -s "${H[@]}" -X POST $BASE/api/auth/setup -d '{"password":"another-pass"}')" "already exists"
check "wrong password" "$(curl -s "${H[@]}" -X POST $BASE/api/auth/login -d '{"password":"nope-nope"}')" "Wrong password"
rm -f "$JAR"; JAR=$(mktemp)
check "login" "$(curl -s -c "$JAR" "${H[@]}" -X POST $BASE/api/auth/login -d '{"password":"correct-horse"}')" '"ok":true'
check "cookie is HttpOnly" "$(cat "$JAR")" "#HttpOnly_"
check "status after login" "$(curl -s -b "$JAR" $BASE/api/auth/status)" '"authenticated":true'
check "SPA index served" "$(curl -s -b "$JAR" $BASE/)" '<div id="root">'
check "state change without CSRF header -> 403" "$(curl -s -o /dev/null -w '%{http_code}' -b "$JAR" -X POST $BASE/api/invoke/file_exists -d '{}')" "403"

echo "== projects"
CFG=$(curl -s -b "$JAR" $BASE/api/web/config)
check "web config" "$CFG" '"projectsDir"'
PROJ_ROOT="$DATA/projects"
R=$(inv create_project "{\"name\":\"demo\",\"path\":\"$PROJ_ROOT\"}")
check "create_project" "$R" '"name":"demo"'
PROJ="$PROJ_ROOT/demo"
check "open_project" "$(inv open_project "{\"path\":\"$PROJ\"}")" '"name":"demo"'
check "fs/list shows project" "$(curl -s -b "$JAR" "$BASE/api/fs/list?path=$PROJ_ROOT")" '"isProject":true'

echo "== upload + import"
UP=$(curl -s -b "$JAR" -H "X-LLM-Wiki: 1" -F "relpath=$(basename "$MD")" -F "file=@$MD" -F "relpath=$(basename "$PDF")" -F "file=@$PDF" $BASE/api/upload)
check "upload" "$UP" '"paths"'
UMD=$(python3 -c 'import json,sys; print(json.loads(sys.argv[1])["paths"][0])' "$UP")
UPDF=$(python3 -c 'import json,sys; print(json.loads(sys.argv[1])["paths"][1])' "$UP")
check "copy md into raw/sources" "$(inv copy_file "{\"source\":\"$UMD\",\"destination\":\"$PROJ/raw/sources/$(basename "$MD")\"}")" "null"
check "copy pdf into raw/sources" "$(inv copy_file "{\"source\":\"$UPDF\",\"destination\":\"$PROJ/raw/sources/$(basename "$PDF")\"}")" "null"
check "preprocess pdf" "$(inv preprocess_file "{\"path\":\"$PROJ/raw/sources/$(basename "$PDF")\"}")" "attention"
check "list_directory" "$(inv list_directory "{\"path\":\"$PROJ/raw/sources\"}")" "$(basename "$PDF")"
check "read md" "$(inv read_file "{\"path\":\"$PROJ/raw/sources/$(basename "$MD")\"}")" "Retrieval"
check "read pdf (pdfium text)" "$(inv read_file "{\"path\":\"$PROJ/raw/sources/$(basename "$PDF")\"}")" "self-attention"
check "write_file" "$(inv write_file "{\"path\":\"$PROJ/wiki/rag.md\",\"contents\":\"# RAG\\n\\nRetrieval augmented generation with vector search.\\n\"}")" "null"
check "file_exists" "$(inv file_exists "{\"path\":\"$PROJ/wiki/rag.md\"}")" "true"
check "search_project" "$(inv search_project "{\"projectPath\":\"$PROJ\",\"query\":\"retrieval\"}")" "rag.md"
check "asset endpoint" "$(curl -s -b "$JAR" "$BASE/api/asset?path=$PROJ/wiki/rag.md")" "Retrieval augmented"
check "asset Range request" "$(curl -s -o /dev/null -w '%{http_code}' -b "$JAR" -H 'Range: bytes=0-3' "$BASE/api/asset?path=$PROJ/raw/sources/$(basename "$PDF")")" "206"

echo "== sandbox"
check "read outside DATA_DIR refused" "$(inv read_file '{"path":"/etc/passwd"}')" "outside the data directory"
check "traversal refused" "$(inv read_file "{\"path\":\"$PROJ/../../../../etc/passwd\"}")" "outside the data directory"
check "asset outside refused" "$(curl -s -o /dev/null -w '%{http_code}' -b "$JAR" "$BASE/api/asset?path=/etc/passwd")" "403"
check "upload dir outside refused" "$(curl -s -o /dev/null -w '%{http_code}' -b "$JAR" -H 'X-LLM-Wiki: 1' -F "file=@$MD" "$BASE/api/upload?dir=/tmp")" "403"
check "unknown command 404" "$(curl -s -o /dev/null -w '%{http_code}' -b "$JAR" "${H[@]}" -X POST $BASE/api/invoke/nope -d '{}')" "404"
check "CLI spawn disabled" "$(inv claude_cli_spawn '{}')" "disabled in the web version"
check "claude detect synthetic" "$(inv claude_cli_detect '{}')" '"installed":false'

echo "== store"
check "store set" "$(curl -s -b "$JAR" "${H[@]}" -X POST $BASE/api/store -d '{"name":"app-state.json","op":"set","key":"smoke","value":{"a":1}}')" '"value":true'
check "store get" "$(curl -s -b "$JAR" "${H[@]}" -X POST $BASE/api/store -d '{"name":"app-state.json","op":"get","key":"smoke"}')" '"a":1'
check "store persisted to app/app-state.json" "$(cat "$DATA/app/app-state.json")" '"smoke"'
check "store bad name refused" "$(curl -s -b "$JAR" "${H[@]}" -X POST $BASE/api/store -d '{"name":"../x","op":"get","key":"k"}')" "invalid store name"

echo "== events (SSE)"
SSE=$(timeout 3 curl -s -N -b "$JAR" $BASE/api/events)
check "SSE connects" "$SSE" "event: ready"
( sleep 1; curl -s -b "$JAR" "${H[@]}" -X POST $BASE/api/emit -d '{"event":"smoke-event","payload":{"n":42}}' >/dev/null ) &
SSE=$(timeout 3 curl -s -N -b "$JAR" $BASE/api/events)
check "SSE delivers emitted event" "$SSE" '"event":"smoke-event"'
check "SSE without session -> 401" "$(curl -s -o /dev/null -w '%{http_code}' $BASE/api/events)" "401"

echo "== proxy"
HDRS=$(printf '[["authorization","Bearer sk-invalid-smoke-test"],["content-type","application/json"]]' | base64 -w0)
P=$(curl -s -D - -b "$JAR" -H "X-LLM-Wiki: 1" -H "X-Proxy-Url: https://api.openai.com/v1/chat/completions" -H "X-Proxy-Headers: $HDRS" -X POST --data '{"model":"gpt-4o-mini","messages":[{"role":"user","content":"hi"}],"stream":true}' $BASE/api/proxy)
check "proxy reaches provider (provider's 401)" "$P" "x-proxy-status: 401"
check "provider error body passed through" "$P" "invalid_api_key\|Incorrect API key"
check "proxy SSRF guard" "$(curl -s -b "$JAR" -H 'X-LLM-Wiki: 1' -H 'X-Proxy-Url: http://127.0.0.1:7860/api/auth/status' $BASE/api/proxy)" "private/loopback"
check "proxy metadata IP blocked" "$(curl -s -b "$JAR" -H 'X-LLM-Wiki: 1' -H 'X-Proxy-Url: http://169.254.169.254/latest/meta-data' $BASE/api/proxy)" "private/loopback"

echo "== clip server via /api/clip"
check "clip /status via proxy" "$(curl -s -b "$JAR" -H 'X-LLM-Wiki: 1' $BASE/api/clip/status)" "ok"

echo "== logout"
check "logout" "$(curl -s -b "$JAR" -c "$JAR" "${H[@]}" -X POST $BASE/api/auth/logout)" '"ok":true'
check "after logout -> 401" "$(curl -s -o /dev/null -w '%{http_code}' -b "$JAR" "${H[@]}" -X POST $BASE/api/invoke/file_exists -d '{}')" "401"

rm -f "$JAR"
echo
echo "passed: $pass   failed: $fail"
[ "$fail" -eq 0 ]

#!/usr/bin/env python3
"""Smallest MCP server that is still a real one: stdio, JSON-RPC 2.0.

Exists so the client's wire format is tested against something that answers,
rather than only against itself. Covers the three things the client does --
initialize, tools/list, tools/call -- plus the two shapes a call can come back
in (ok, and isError). Also logs to stderr, so the stderr drain is exercised.
"""
import json
import sys

TOOLS = [
    {
        "name": "echo.it",              # '.' is legal here, illegal in a function name
        "description": "Echo the text back",
        "inputSchema": {
            "type": "object",
            "properties": {"text": {"type": "string"}},
            "required": ["text"],
        },
    },
    {
        "name": "boom",
        "description": "Always fails",
        "inputSchema": {"type": "object", "properties": {}},
    },
]


def reply(msg_id, result):
    sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": msg_id, "result": result}) + "\n")
    sys.stdout.flush()


print("fake mcp server up", file=sys.stderr, flush=True)

for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    req = json.loads(line)
    method, msg_id = req.get("method"), req.get("id")

    if msg_id is None:
        continue  # a notification: nothing to answer

    if method == "initialize":
        reply(msg_id, {
            "protocolVersion": "2024-11-05",
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "fake", "version": "0"},
        })
    elif method == "tools/list":
        reply(msg_id, {"tools": TOOLS})
    elif method == "tools/call":
        params = req.get("params", {})
        name = params.get("name")
        args = params.get("arguments", {})
        if name == "echo.it":
            reply(msg_id, {"content": [{"type": "text", "text": f"echo: {args.get('text', '')}"}]})
        elif name == "boom":
            reply(msg_id, {"content": [{"type": "text", "text": "it broke"}], "isError": True})
        else:
            sys.stdout.write(json.dumps({
                "jsonrpc": "2.0", "id": msg_id,
                "error": {"code": -32602, "message": f"no such tool: {name}"},
            }) + "\n")
            sys.stdout.flush()
    else:
        sys.stdout.write(json.dumps({
            "jsonrpc": "2.0", "id": msg_id,
            "error": {"code": -32601, "message": f"no such method: {method}"},
        }) + "\n")
        sys.stdout.flush()

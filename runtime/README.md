# Darling Runtime (Python)

This directory contains the external agent runtime that handles planner + worker execution using the OpenAI Agents SDK.

## Install

```bash
pip install -r requirements.txt
```

## Configure

Required:
- `OPENAI_API_KEY`

Optional:
- `DARLING_RUNTIME_PORT` (default 3999)

The Rust app will call this runtime at `DARLING_RUNTIME_URL` (see root `.env.example`).

## Run

```bash
cd runtime
python app.py
```

## Protocol

The runtime exposes:
- `POST /run`

Payload is provided by the Rust layer and includes `input`, `context`, `run_id`, `mcp_server_command`, `hard_policy`, and `run_dir`.

The runtime starts the Rust MCP stdio server process using `mcp_server_command`, then calls tools over MCP JSON-RPC.

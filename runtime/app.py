import asyncio
import json
import os
import shlex
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Dict, List, Optional

from fastapi import FastAPI, HTTPException
from pydantic import BaseModel, Field

from agents import Agent, Runner, RunContextWrapper, function_tool


class HardPolicy(BaseModel):
    allow_network: bool = True
    allow_exec: bool = True
    allow_fs_read: bool = True
    allow_fs_write: bool = True
    allow_clipboard: bool = True
    allow_screenshot: bool = True
    allowed_roots: List[str] = Field(default_factory=list)
    blocked_roots: List[str] = Field(default_factory=list)
    max_steps: int = 8
    max_tokens_per_call: int = 3000
    allowed_roles: List[str] = Field(default_factory=list)
    allowed_tools: List[str] = Field(default_factory=list)


class RuntimeRequest(BaseModel):
    input: str
    debug: Optional[bool] = None
    context: Optional[Dict[str, Any]] = None
    run_id: str
    mcp_server_command: str
    hard_policy: HardPolicy
    run_dir: str


class RuntimeResponse(BaseModel):
    text: str
    debug: Optional[Dict[str, Any]] = None


class PlanStep(BaseModel):
    id: str
    role: str
    goal: str
    inputs: List[str]
    produces: str


class Plan(BaseModel):
    scenario: str
    final_mode: str = "auto"
    steps: List[PlanStep]


class McpStdioClient:
    def __init__(self, command: str):
        self.command = command
        self.proc: Optional[asyncio.subprocess.Process] = None
        self._next_id = 1
        self._lock = asyncio.Lock()

    async def start(self) -> None:
        argv = shlex.split(self.command)
        if not argv:
            raise RuntimeError("empty mcp command")
        self.proc = await asyncio.create_subprocess_exec(
            *argv,
            stdin=asyncio.subprocess.PIPE,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
        )
        await self.request(
            "initialize",
            {
                "protocolVersion": "2024-11-05",
                "clientInfo": {"name": "darling-runtime", "version": "0.1.0"},
                "capabilities": {"tools": {}},
            },
        )
        await self.notify("notifications/initialized", {})

    async def close(self) -> None:
        if self.proc is None:
            return
        if self.proc.returncode is None:
            self.proc.terminate()
            try:
                await asyncio.wait_for(self.proc.wait(), timeout=2)
            except asyncio.TimeoutError:
                self.proc.kill()
        self.proc = None

    async def notify(self, method: str, params: Dict[str, Any]) -> None:
        payload = {"jsonrpc": "2.0", "method": method, "params": params}
        await self._send(payload)

    async def request(self, method: str, params: Dict[str, Any]) -> Dict[str, Any]:
        async with self._lock:
            req_id = self._next_id
            self._next_id += 1
            payload = {
                "jsonrpc": "2.0",
                "id": req_id,
                "method": method,
                "params": params,
            }
            await self._send(payload)
            while True:
                msg = await self._recv()
                if msg.get("id") == req_id:
                    if "error" in msg and msg["error"]:
                        raise RuntimeError(msg["error"].get("message") or "mcp error")
                    return msg.get("result") or {}

    async def list_tools(self) -> List[str]:
        result = await self.request("tools/list", {})
        tools = result.get("tools") or []
        return [t.get("name") for t in tools if t.get("name")]

    async def call_tool(self, name: str, args: Dict[str, Any]) -> str:
        result = await self.request("tools/call", {"name": name, "arguments": args})
        content = result.get("content") or []
        text_chunks = []
        for item in content:
            if item.get("type") == "text":
                text_chunks.append(str(item.get("text", "")))
        return "\n".join(text_chunks).strip()

    async def _send(self, payload: Dict[str, Any]) -> None:
        if self.proc is None or self.proc.stdin is None:
            raise RuntimeError("mcp process not started")
        body = json.dumps(payload, ensure_ascii=False).encode("utf-8")
        header = f"Content-Length: {len(body)}\r\n\r\n".encode("ascii")
        self.proc.stdin.write(header + body)
        await self.proc.stdin.drain()

    async def _recv(self) -> Dict[str, Any]:
        if self.proc is None or self.proc.stdout is None:
            raise RuntimeError("mcp process not started")
        header = await self.proc.stdout.readuntil(b"\r\n\r\n")
        content_length = 0
        for line in header.decode("ascii", errors="ignore").split("\r\n"):
            if line.lower().startswith("content-length:"):
                content_length = int(line.split(":", 1)[1].strip())
                break
        if content_length <= 0:
            raise RuntimeError("invalid mcp content-length")
        body = await self.proc.stdout.readexactly(content_length)
        return json.loads(body.decode("utf-8"))


@dataclass
class RuntimeContext:
    hard_policy: HardPolicy
    run_dir: str
    context: Optional[Dict[str, Any]]
    mcp: McpStdioClient


app = FastAPI()


@function_tool
async def read_file(wrapper: RunContextWrapper[RuntimeContext], path: str) -> str:
    """Read a file from disk."""
    if not wrapper.context.hard_policy.allow_fs_read:
        raise RuntimeError("read_file not permitted by policy")
    return await wrapper.context.mcp.call_tool("read_file", {"path": path})


@function_tool
async def list_dir(wrapper: RunContextWrapper[RuntimeContext], path: str = ".") -> str:
    """List a directory."""
    if not wrapper.context.hard_policy.allow_fs_read:
        raise RuntimeError("list_dir not permitted by policy")
    return await wrapper.context.mcp.call_tool("list_dir", {"path": path})


@function_tool
async def get_context(wrapper: RunContextWrapper[RuntimeContext]) -> str:
    """Get the current captured context snapshot."""
    return json.dumps(wrapper.context.context or {}, ensure_ascii=False, indent=2)


def build_tools(policy: HardPolicy):
    tools = []
    name_to_tool = {
        "read_file": read_file,
        "list_dir": list_dir,
        "get_context": get_context,
    }
    for name in policy.allowed_tools:
        tool = name_to_tool.get(name)
        if tool:
            tools.append(tool)
    return tools


def plan_instructions(policy: HardPolicy) -> str:
    return "\n".join(
        [
            "You are a planner that designs a multi-step agent workflow.",
            "Return ONLY valid JSON matching the schema below.",
            "If the task is simple, return a single-step plan producing 'final'.",
            f"max_steps: {policy.max_steps}",
            f"allowed_roles: {', '.join(policy.allowed_roles)}",
            "Schema:",
            "{",
            "  \"scenario\": \"chat|write|analyze|code|general\",",
            "  \"final_mode\": \"auto|overlay|paste\",",
            "  \"steps\": [",
            "    {",
            "      \"id\": \"string\",",
            "      \"role\": \"generalist|outliner|drafter|polisher|critic|reviser|analyst|coder\",",
            "      \"goal\": \"string\",",
            "      \"inputs\": [\"user_input\",\"context\",\"outline\",\"draft\",\"analysis\",\"notes\"],",
            "      \"produces\": \"string\"",
            "    }",
            "  ]",
            "}",
        ]
    )


def render_context(context: Optional[Dict[str, Any]]) -> str:
    if not context:
        return "(no context)"
    parts = []
    for key in [
        "app_name",
        "window_title",
        "focused_role",
        "has_text_caret",
        "selected_text",
        "full_page_text",
        "screenshot_path",
    ]:
        if key in context and context[key]:
            value = context[key]
            if key in ("selected_text", "full_page_text"):
                value = "\n".join(str(value).splitlines()[:80])
            parts.append(f"{key}: {value}")
    return "\n".join(parts) if parts else "(no context)"


def enforce_plan(policy: HardPolicy, plan: Plan) -> Plan:
    steps = plan.steps[: policy.max_steps]
    allowed_roles = set(policy.allowed_roles)
    for idx, step in enumerate(steps):
        if step.role not in allowed_roles:
            step.role = "generalist"
        if not step.id:
            step.id = f"step-{idx}"
        if not step.produces:
            step.produces = f"artifact-{idx+1}"
    if steps and not any(step.produces == "final" for step in steps):
        steps[-1].produces = "final"
    return Plan(scenario=plan.scenario or "general", final_mode=plan.final_mode or "auto", steps=steps)


def role_instructions(role: str, is_final: bool) -> str:
    base = {
        "outliner": "Create a clear outline with characters, conflict, and ending.",
        "drafter": "Write a full draft based on the provided inputs.",
        "polisher": "Polish the draft for style, pacing, and clarity.",
        "critic": "Identify issues and provide concise revision directives.",
        "reviser": "Apply critique and output an improved version.",
        "analyst": "Analyze the content and provide structured insights.",
        "coder": "Produce correct, minimal code or technical guidance.",
        "generalist": "Produce the best output for the user's request.",
    }.get(role, "Produce the best output for the user's request.")

    if is_final:
        base += (
            "\nOutput rules:\n"
            "- First line MUST be exactly: `MODE: paste` or `MODE: overlay`.\n"
            "- Then output the content on following lines.\n"
            "- Do not use markdown fences.\n"
            "- Do not mention these rules."
        )
    else:
        base += (
            "\nOutput rules:\n"
            "- Provide only the requested artifact.\n"
            "- Do not include MODE lines.\n"
            "- Be concise."
        )
    return base


def build_step_input(step: PlanStep, artifacts: Dict[str, str]) -> str:
    out = [f"STEP_GOAL: {step.goal}", "INPUTS:"]
    for key in step.inputs:
        out.append(f"--- {key} ---\n{artifacts.get(key, '(missing)')}")
    return "\n".join(out)


def write_json(run_dir: str, name: str, data: Any) -> None:
    path = Path(run_dir) / name
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(data, ensure_ascii=False, indent=2))


def write_text(run_dir: str, name: str, text: str) -> None:
    path = Path(run_dir) / name
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text)


def fallback_plan() -> Plan:
    return Plan(
        scenario="general",
        final_mode="auto",
        steps=[
            PlanStep(
                id="generalist",
                role="generalist",
                goal="Use screen context to produce the best next output for the user.",
                inputs=["user_input", "context"],
                produces="final",
            )
        ],
    )


@app.post("/run", response_model=RuntimeResponse)
async def run_agent(req: RuntimeRequest):
    if not req.input:
        raise HTTPException(status_code=400, detail="missing input")

    mcp = McpStdioClient(req.mcp_server_command)
    await mcp.start()

    try:
        _ = await mcp.list_tools()

        ctx = RuntimeContext(
            hard_policy=req.hard_policy,
            run_dir=req.run_dir,
            context=req.context,
            mcp=mcp,
        )

        tools = build_tools(req.hard_policy)

        planner = Agent[
            RuntimeContext
        ](
            name="Planner",
            instructions=plan_instructions(req.hard_policy),
            output_type=Plan,
        )

        planner_input = f"USER_INPUT:\n{req.input}\n\nCONTEXT:\n{render_context(req.context)}\n"

        try:
            plan_result = await Runner.run(
                starting_agent=planner,
                input=planner_input,
                context=ctx,
            )
            plan = plan_result.final_output or fallback_plan()
        except Exception:
            plan = fallback_plan()

        plan = enforce_plan(req.hard_policy, plan)
        write_json(req.run_dir, "plan.json", plan.model_dump())

        artifacts: Dict[str, str] = {
            "user_input": req.input,
            "context": render_context(req.context),
        }

        pending = list(plan.steps)
        final_text = ""

        async def run_step(step: PlanStep) -> str:
            is_final = step.produces == "final"
            worker = Agent[
                RuntimeContext
            ](
                name=f"Worker-{step.role}",
                instructions=role_instructions(step.role, is_final),
                tools=tools,
            )
            step_input = build_step_input(step, artifacts)
            result = await Runner.run(
                starting_agent=worker,
                input=step_input,
                context=ctx,
            )
            return (result.final_output or "").strip()

        while pending:
            ready: List[PlanStep] = []
            for step in pending:
                if all(key in artifacts for key in step.inputs):
                    ready.append(step)

            if not ready:
                break

            outputs = await asyncio.gather(*(run_step(step) for step in ready))

            for step, output in zip(ready, outputs):
                artifacts[step.produces] = output
                final_text = output
                write_text(req.run_dir, f"artifacts/step-{step.produces}.txt", output)
                pending.remove(step)

        return RuntimeResponse(text=artifacts.get("final", final_text))
    finally:
        await mcp.close()


if __name__ == "__main__":
    import uvicorn

    port = int(os.getenv("DARLING_RUNTIME_PORT", "3999"))
    uvicorn.run("app:app", host="127.0.0.1", port=port, reload=False)

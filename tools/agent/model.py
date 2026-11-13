"""Stateless Responses streaming. No request/response or tool-round length cap."""
import json
import urllib.error
import urllib.parse
import urllib.request


def function(name, description, properties, required):
    return {"type": "function", "name": name, "description": description, "strict": True,
            "parameters": {"type": "object", "properties": properties, "required": required, "additionalProperties": False}}


def tools(grants):
    entries = []
    if grants & 1:
        entries.append(function("device_status", "Read actual battery, volume and brightness.", {}, []))
    for name, bit, low in (("set_volume", 2, 0), ("set_brightness", 4, 10)):
        if grants & bit:
            entries.append(function(name, "Set the device percentage through its system API.",
                                    {"percent": {"type": "integer", "minimum": low, "maximum": 100}}, ["percent"]))
    names = [t["name"] for t in entries]
    entries += [function("list_skills", "List already installed skills; reuse an existing matching skill.", {}, []),
                function("create_skill", "Validate, simulate and atomically install a new zero-argument automation skill. This does not execute it.",
                         {"id": {"type": "string"}, "description": {"type": "string"},
                          "steps": {"type": "array", "minItems": 1, "items": {"type": "object", "properties": {
                              "tool": {"type": "string", "enum": names}, "percent": {"type": ["integer", "null"]}},
                              "required": ["tool", "percent"], "additionalProperties": False}}}, ["id", "description", "steps"]),
                function("run_skill", "Execute a validated installed skill using only granted device APIs.", {"id": {"type": "string"}}, ["id"])]
    return entries


def visible(output):
    return "".join(content["text"] for item in output if item.get("type") == "message"
                   for content in item.get("content", []) if content.get("type") == "output_text")


def events(response):
    data = []
    for raw in response:
        line = raw.decode("utf-8").rstrip("\r\n")
        if not line:
            if data:
                value = "\n".join(data)
                data.clear()
                if value != "[DONE]":
                    yield json.loads(value)
        elif line.startswith("data:"):
            data.append(line[5:].removeprefix(" "))
    if data:
        raise ValueError("Responses event ended without a framing terminator")


class Responses:
    def __init__(self, config):
        chat = config["chat"]
        parsed = urllib.parse.urlsplit(chat["url"])
        if parsed.scheme != "https" or not parsed.hostname or parsed.username or parsed.password or parsed.fragment:
            raise ValueError("Responses needs an HTTPS endpoint without URL credentials")
        self.url, self.model, self.key = chat["url"], chat["model"], chat["api_key"]
        if not all(isinstance(x, str) and x for x in (self.model, self.key)) or any(c in self.key for c in "\r\n\0"):
            raise ValueError("invalid private model configuration")
        self.prompt = config.get("system_prompt", "你是团子的设备助手。依据工具的实际结果回复，使用中文。")

    def round(self, context, available_tools, emit, check_cancel):
        check_cancel()
        body = json.dumps({"model": self.model, "instructions": self.prompt,
                           "input": context, "stream": True, "store": False, "tools": available_tools}, ensure_ascii=False).encode()
        request = urllib.request.Request(self.url, data=body, headers={"Authorization": "Bearer " + self.key,
                    "Content-Type": "application/json", "Accept": "text/event-stream", "Accept-Encoding": "identity"})
        try:
            # Timeout detects a stalled transport; it is not a task duration cap.
            with urllib.request.urlopen(request, timeout=30) as response:
                if response.headers.get_content_type() != "text/event-stream":
                    raise ValueError("provider did not return SSE")
                streamed = ""
                completed = None
                for event in events(response):
                    check_cancel()
                    kind = event.get("type")
                    if kind == "response.output_text.delta":
                        delta = event["delta"]
                        streamed += delta
                        emit(delta)
                    elif kind == "response.completed":
                        result = event["response"]
                        if result.get("status") != "completed" or result.get("error") is not None or result.get("incomplete_details") is not None:
                            raise ValueError("provider did not complete the response")
                        completed = result["output"]
                        break
                    elif kind in ("response.failed", "response.incomplete", "error"):
                        raise ValueError("provider failed or returned an incomplete response")
                if completed is None:
                    raise ValueError("provider stream ended before response.completed")
                final = visible(completed)
                if streamed:
                    if final != streamed:
                        raise ValueError("streamed text differs from the completed response")
                elif final:
                    emit(final)
                for item in completed:
                    if item.get("type") == "function_call" and item.get("status") != "completed":
                        raise ValueError("provider returned an incomplete function call")
                check_cancel()
                return completed
        except urllib.error.HTTPError as error:
            # Never include request headers, keys or provider bodies in errors.
            raise RuntimeError(f"model HTTP status {error.code}") from None

"""Exclusive USB transport, task cancellation and a single Responses scheduler."""
import json
import re
import secrets
import threading
import time
from . import model
from .skills import CAPABILITIES, Store, action


class Cancelled(Exception):
    pass


def strict_json(text):
    def pairs(items):
        result = {}
        for key, value in items:
            if key in result:
                raise ValueError("duplicate JSON field")
            result[key] = value
        return result
    return json.loads(text, object_pairs_hook=pairs, parse_constant=lambda _: (_ for _ in ()).throw(ValueError("invalid JSON constant")))


def utf8_chunks(text, limit=100):
    if "\0" in text:
        raise ValueError("visible reply contains NUL")
    raw = text.encode()
    while raw:
        end = min(len(raw), limit)
        while end < len(raw) and raw[end] & 0xC0 == 0x80:
            end -= 1
        if not end:
            raise ValueError("transport chunk cannot hold one Unicode character")
        yield raw[:end]
        raw = raw[end:]


class Wire:
    def __init__(self, device, grants):
        self.device, self.grants = device, grants
        self.nonce = secrets.randbelow(0xFFFFFFFF) + 1
        self.lock = threading.Lock()
        self.command(f"agent-attach {self.nonce} {grants}")

    def command(self, text):
        # The serial port has one owner; heartbeat and model worker serialize
        # requests here. Never retry a command after an ambiguous transport ACK.
        with self.lock:
            try:
                reply = self.device.command(text, timeout=20, check=False)
            except SystemExit:
                raise OSError("device command timed out") from None
        if not reply.startswith("@ok "):
            raise OSError(reply)
        if reply.split()[1] != text.split()[0]:
            raise OSError("unexpected device acknowledgement; connection cannot be reused")
        return reply

    def poll(self):
        reply = self.command(f"agent-poll {self.nonce}")
        match = re.fullmatch(r"@ok agent-poll id=(\d+) bytes=(\d+) output=(\d+)", reply)
        if not match:
            raise ValueError("invalid agent-poll reply")
        return tuple(map(int, match.groups()))

    def input(self, identity, total):
        raw = bytearray()
        while len(raw) < total:
            reply = self.command(f"agent-input {self.nonce} {identity} {len(raw)}")
            chunk = bytes.fromhex(reply.removeprefix("@ok agent-input hex="))
            if not chunk or len(chunk) > 40 or len(raw) + len(chunk) > total:
                raise ValueError("invalid agent input progress")
            raw.extend(chunk)
        return raw.decode()

    def heartbeat(self):
        self.command(f"agent-heartbeat {self.nonce}")

    def finish(self, identity, ok):
        self.command(f"agent-done {self.nonce} {identity} {'ok' if ok else 'error'}")

    def detach(self):
        self.command(f"agent-detach {self.nonce}")


class Task:
    def __init__(self, wire, identity, cancel):
        self.wire, self.identity, self.cancel = wire, identity, cancel
        self.offset, self.call = 0, 0
        self.receipt = {"task": identity, "backend": "desktop Responses", "tools": [], "rounds": 0}

    def check(self):
        if self.cancel.is_set():
            raise Cancelled("task cancelled or device disconnected")

    def emit(self, text):
        for raw in utf8_chunks(text):
            self.check()
            reply = self.wire.command(f"agent-feed {self.wire.nonce} {self.identity} {self.offset} {raw.hex()}")
            self.offset += len(raw)
            if reply != f"@ok agent-feed bytes={self.offset}":
                raise ValueError("invalid agent output acknowledgement")

    def execute(self, name, arguments):
        self.check()
        name, arguments = action(name, arguments)
        if CAPABILITIES[name] & self.wire.grants == 0:
            raise ValueError("device capability denied")
        verb = {"device_status": "status", "set_volume": "volume", "set_brightness": "brightness"}[name]
        self.call += 1
        suffix = "" if not arguments else " " + str(arguments["percent"])
        reply = self.wire.command(f"agent-tool {self.wire.nonce} {self.identity} {self.call} {verb}{suffix}")
        match = re.fullmatch(r"@ok agent-tool call=(\d+) bytes=(\d+)", reply)
        if not match or int(match[1]) != self.call:
            raise ValueError("invalid agent tool acknowledgement")
        total, raw = int(match[2]), bytearray()
        while len(raw) < total:
            self.check()
            reply = self.wire.command(f"agent-result {self.wire.nonce} {self.identity} {self.call} {len(raw)}")
            chunk = bytes.fromhex(reply.removeprefix("@ok agent-result hex="))
            if not chunk or len(chunk) > 40 or len(raw) + len(chunk) > total:
                raise ValueError("invalid agent result progress")
            raw.extend(chunk)
        result = strict_json(raw.decode())
        self.receipt["tools"].append({"name": name, "arguments": arguments, "result": result})
        self.check()
        return result


def run(task, backend, store, text):
    context = [{"type": "message", "role": "user", "content": text}]
    available = model.tools(task.wire.grants)
    seen = {}
    while True:
        task.check()
        output = backend.round(context, available, task.emit, task.check)
        task.receipt["rounds"] += 1
        context.extend(output)  # Includes provider reasoning/encrypted content.
        calls = [item for item in output if item.get("type") == "function_call"]
        if not calls:
            return task.receipt
        for call in calls:
            task.check()
            identity = call["call_id"]
            fingerprint = (call["name"], call["arguments"])
            if not isinstance(identity, str) or not identity:
                raise ValueError("invalid function call identity")
            if identity in seen:
                if seen[identity][0] != fingerprint:
                    raise ValueError("provider reused a call identity for a different action")
                result = seen[identity][1]
            else:
                try:
                    args = strict_json(call["arguments"])
                    name = call["name"]
                    if name == "create_skill":
                        result = store.install(args, task.check)
                    elif name == "list_skills" and args == {}:
                        result = {"ok": True, "skills": store.list()}
                    elif name == "run_skill" and isinstance(args, dict) and set(args) == {"id"}:
                        result = store.run(args["id"], task.execute, task.check)
                    else:
                        result = task.execute(name, args)
                except (ValueError, KeyError, TypeError) as error:
                    result = {"ok": False, "error": str(error)}
                task.check()
                seen[identity] = (fingerprint, result)
            task.receipt.setdefault("calls", []).append({"id": identity, "name": call["name"], "result": result})
            context.append({"type": "function_call_output", "call_id": identity,
                            "output": json.dumps(result, ensure_ascii=False)})


def serve(device, config, skills_root, grants=7, once=None, on_complete=None):
    wire = Wire(device, grants)
    backend, store = model.Responses(config), Store(skills_root, grants)
    active, worker, cancellation = None, None, None
    finished, handled = {}, None
    results = []
    try:
        if once is not None:
            from console import voice_ask_command
            wire.command(voice_ask_command(once))
        heartbeat = time.monotonic()
        while True:
            if time.monotonic() - heartbeat >= 1:
                wire.heartbeat()
                heartbeat = time.monotonic()
            identity, total, _ = wire.poll()
            if active is not None and identity != active:
                cancellation.set()
            if active is not None and active in finished:
                receipt = finished.pop(active)
                results.append(receipt)
                if on_complete:
                    on_complete(receipt)
                handled, active = active, None
                if once is not None:
                    if receipt["state"] != "done":
                        raise RuntimeError(receipt["error"])
                    return results
            if active is None and identity and identity != handled:
                if worker is not None and worker.is_alive():
                    # An old network read may finish after cancellation. It
                    # never regains authority and cannot overlap new actions.
                    time.sleep(.05)
                    continue
                text = wire.input(identity, total)
                active, cancellation = identity, threading.Event()
                task = Task(wire, identity, cancellation)
                def work(task=task, text=text):
                    try:
                        receipt = run(task, backend, store, text)
                        task.check()
                        wire.finish(task.identity, True)
                        receipt["state"] = "done"
                    except Exception as error:
                        receipt = task.receipt
                        receipt.update(state="cancelled" if isinstance(error, Cancelled) else "error", error=type(error).__name__ + ": " + str(error))
                        if not task.cancel.is_set():
                            try:
                                wire.finish(task.identity, False)
                            except OSError:
                                pass
                    receipt["reply_bytes"] = task.offset
                    finished[task.identity] = receipt
                worker = threading.Thread(target=work, daemon=True)
                worker.start()
            time.sleep(.05)
    finally:
        if cancellation:
            cancellation.set()
        try:
            wire.detach()
        except (OSError, ValueError):
            pass
        # A blocked model read has its own transport timeout. It holds no
        # device authority after cancellation and this function never retries.

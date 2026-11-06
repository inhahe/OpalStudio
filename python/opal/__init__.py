"""Drive Opal Studio from Python.

Connect to the editor running on this computer, then call its commands and queries as
methods: ::

    import opal

    editor = opal.connect()
    doc = editor.new(width=1920, height=1080)["document"]
    editor.new_layer(name="Sky")
    editor.export(path="sky.png")

Each method is one of the editor's commands (named as in the editor, in snake case) or a
query (``documents``, ``layers``, ``image``), taking its parameters by name (paths
relative to the current folder, as Python's own ``open`` takes them); ``editor.methods()``
lists them all, ``opal/__init__.pyi`` describes them for editors and type checkers, and
``API.md`` documents them. A command that fails raises :class:`OpalError` with the
editor's reason.

What a script changes in an image becomes one history state when it ends (or
disconnects), so that one undo takes back the whole run; in an interactive session each
call is a step of its own.

The connection is the editor's local pipe, found with its token in ``api.json`` in the
user's settings folder; only the user's own programs can read it.
"""

from __future__ import annotations

import itertools
import json
import operator
import os
import sys
import time
from pathlib import Path
from typing import Any

__all__ = ["Editor", "OpalError", "connect", "connection_file"]

#: The protocol version this package speaks.
PROTOCOL = 1


class OpalError(Exception):
    """A call the editor refused or could not carry out, with its reason."""

    def __init__(self, message: str, code: int | None = None):
        super().__init__(message)
        self.code = code


def connection_file() -> Path:
    """Where the running editor writes its pipe's name and token: ``OPAL_STUDIO_API`` if it
    is set (as it is for a script the editor runs), otherwise the user's settings folder
    (as the editor's `directories` crate names it; only Windows is built and tried yet)."""
    given = os.environ.get("OPAL_STUDIO_API")
    if given:
        return Path(given)
    if sys.platform == "win32":
        base = Path(os.environ.get("APPDATA", Path.home() / "AppData" / "Roaming"))
        return base / "Opal Studio" / "config" / "api.json"
    if sys.platform == "darwin":
        return Path.home() / "Library" / "Application Support" / "Opal-Studio" / "api.json"
    base = Path(os.environ.get("XDG_CONFIG_HOME", Path.home() / ".config"))
    return base / "opalstudio" / "api.json"


def _running_script() -> str | None:
    """The file name of the script Python is running, if it is running one (not in an
    interactive session or a notebook, which go on as long as the user likes)."""
    if hasattr(sys, "ps1") or sys.flags.interactive or "ipykernel" in sys.modules:
        return None
    main = getattr(sys.modules.get("__main__"), "__file__", None)
    return os.path.basename(main) if main else None


#: ``connect``'s default script name: the running script's, found when it connects.
_RUNNING: Any = object()


def _plain(value: Any) -> Any:
    """What JSON cannot take as it is, as what it stands for: numpy's numbers and arrays (as
    their values and lists), and anything else that is a whole number or a number."""
    if hasattr(value, "tolist"):
        return value.tolist()
    try:
        return operator.index(value)
    except TypeError:
        pass
    try:
        return float(value)
    except (TypeError, ValueError):
        raise TypeError(f"A {type(value).__name__} cannot be sent to the editor.") from None


class _Pipe:
    """The connection: a line out, a line back."""

    def __init__(self, name: str):
        if sys.platform == "win32":
            # While every instance of the pipe is busy (the editor accepting another client
            # just then: ERROR_PIPE_BUSY), it is tried again for a moment.
            for attempt in range(40):
                try:
                    self._file = open(name, "r+b", buffering=0)
                    break
                except OSError as e:
                    if getattr(e, "winerror", None) != 231 or attempt == 39:
                        raise
                    time.sleep(0.025)
            self._recv = self._file.read
            self._send = self._file.write
        else:
            import socket

            sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            # A name without a path is in the abstract namespace (Linux).
            sock.connect(name if "/" in name else "\0" + name)
            self._file = sock
            self._recv = sock.recv
            self._send = sock.sendall
        self._pending = b""

    def exchange(self, message: dict) -> dict:
        """Sends ``message`` and returns the reply to it. Replies to earlier messages still
        on their way (those of a call interrupted while it waited) are passed over."""
        line = (json.dumps(message, default=_plain) + "\n").encode("utf-8")
        try:
            self._send(line)
            while True:
                while b"\n" not in self._pending:
                    chunk = self._recv(65536)
                    if not chunk:
                        raise OpalError("The editor closed the connection.")
                    self._pending += chunk
                line, self._pending = self._pending.split(b"\n", 1)
                reply = json.loads(line)
                # A reply with no id is to a line the editor could not read: this one.
                if reply.get("id") in (message.get("id"), None):
                    return reply
        except OSError:
            # A pipe the editor has dropped (it closed, or the user disconnected scripts).
            raise OpalError("The editor closed the connection.") from None

    def close(self) -> None:
        self._file.close()


class Editor:
    """A connection to a running Opal Studio: its methods are the editor's."""

    def __init__(self, pipe: str, token: str, script: str | None = None):
        try:
            self._pipe = _Pipe(pipe)
        except OSError:
            raise OpalError("Opal Studio is not running (its pipe is gone): start it again.") from None
        self._ids = itertools.count(1)
        try:
            hello = self.call("hello", token=token, **({"script": script} if script else {}))
            if hello.get("protocol") != PROTOCOL:
                raise OpalError(f"The editor speaks protocol {hello.get('protocol')}, this package {PROTOCOL}.")
        except OpalError:
            self._pipe.close()
            raise
        self.version = hello.get("editor")
        self._methods = {m["name"]: m for m in self.call("methods")}

    def call(self, method: str, **params: Any) -> Any:
        """Calls ``method`` with ``params``; returns what it gives back."""
        # Paths go as absolute ones, against this process's current folder (the editor's is
        # another).
        kinds = {p["name"]: p["kind"] for p in self.__dict__.get("_methods", {}).get(method, {}).get("params", [])}
        params = {
            k: os.path.abspath(os.fspath(v)) if kinds.get(k) == "path" and v is not None else v for k, v in params.items()
        }
        reply = self._pipe.exchange({"jsonrpc": "2.0", "id": next(self._ids), "method": method, "params": params})
        if "error" in reply:
            raise OpalError(reply["error"].get("message", "The editor refused."), reply["error"].get("code"))
        return reply.get("result")

    def methods(self) -> list[dict]:
        """Every method, with its help and parameters (as the editor describes them)."""
        return list(self._methods.values())

    def __getattr__(self, name: str):
        methods = self.__dict__.get("_methods", {})
        if name not in methods:
            raise AttributeError(f"The editor has no method {name!r}.")

        def method(**params: Any) -> Any:
            return self.call(name, **params)

        method.__name__ = name
        method.__doc__ = methods[name]["help"]
        return method

    def disconnect(self) -> None:
        """Closes the connection (``close`` is the editor's command, which closes an image)."""
        self._pipe.close()

    def __enter__(self) -> Editor:
        return self

    def __exit__(self, *exc: Any) -> None:
        self.disconnect()


def connect(file: str | os.PathLike | None = None, *, script: str | None = _RUNNING) -> Editor:
    """Connects to the editor running on this computer (as ``file``, or the user's
    :func:`connection_file`, says where).

    The connection is the run of script ``script`` (by default the running script's file
    name): what it changes in each image is joined into one history state, named after it,
    when it disconnects. With ``None`` (the default in an interactive session), each call
    is a history state of its own."""
    path = Path(file) if file is not None else connection_file()
    try:
        info = json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError:
        raise OpalError(f"Opal Studio is not running (there is no {path}).") from None
    return Editor(info["pipe"], info["token"], _running_script() if script is _RUNNING else script)

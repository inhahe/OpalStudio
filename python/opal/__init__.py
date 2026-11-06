"""Drive Opal Studio from Python.

Connect to the editor running on this computer, then call its commands and queries as
methods: ::

    import opal

    editor = opal.connect()
    doc = editor.new(width=1920, height=1080)["document"]
    editor.new_layer(name="Sky")
    editor.export(path="sky.png")

Each method is one of the editor's commands (named as in the editor, in snake case) or a
query (``documents``, ``layers``), taking its parameters by name; ``editor.methods()``
lists them all, ``opal/__init__.pyi`` describes them for editors and type checkers, and
``API.md`` documents them. A command that fails raises :class:`OpalError` with the
editor's reason.

The connection is the editor's local pipe, found with its token in ``api.json`` in the
user's settings folder; only the user's own programs can read it.
"""

from __future__ import annotations

import itertools
import json
import os
import sys
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


class _Pipe:
    """The connection: a line out, a line back."""

    def __init__(self, name: str):
        if sys.platform == "win32":
            self._file = open(name, "r+b", buffering=0)
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
        self._send((json.dumps(message) + "\n").encode("utf-8"))
        while b"\n" not in self._pending:
            chunk = self._recv(65536)
            if not chunk:
                raise OpalError("The editor closed the connection.")
            self._pending += chunk
        line, self._pending = self._pending.split(b"\n", 1)
        return json.loads(line)

    def close(self) -> None:
        self._file.close()


class Editor:
    """A connection to a running Opal Studio: its methods are the editor's."""

    def __init__(self, pipe: str, token: str):
        self._pipe = _Pipe(pipe)
        self._ids = itertools.count(1)
        hello = self.call("hello", token=token)
        if hello.get("protocol") != PROTOCOL:
            raise OpalError(f"The editor speaks protocol {hello.get('protocol')}, this package {PROTOCOL}.")
        self.version = hello.get("editor")
        self._methods = {m["name"]: m for m in self.call("methods")}

    def call(self, method: str, **params: Any) -> Any:
        """Calls ``method`` with ``params``; returns what it gives back."""
        # Paths given as path objects go as text; everything else as it is.
        params = {k: os.fspath(v) if isinstance(v, os.PathLike) else v for k, v in params.items()}
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

    def close(self) -> None:
        self._pipe.close()

    def __enter__(self) -> Editor:
        return self

    def __exit__(self, *exc: Any) -> None:
        self.close()


def connect(file: str | os.PathLike | None = None) -> Editor:
    """Connects to the editor running on this computer (as ``file``, or the user's
    :func:`connection_file`, says where)."""
    path = Path(file) if file is not None else connection_file()
    try:
        info = json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError:
        raise OpalError(f"Opal Studio is not running (there is no {path}).") from None
    return Editor(info["pipe"], info["token"])

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

``editor.pixels()`` gives an image's pixels (or a layer's) as a numpy array, and
``editor.put_pixels(array)`` puts some in, as a new layer or into a layer's pixels; on
Windows they pass through shared memory rather than the connection. ::

    sky = editor.pixels(format="linear")          # height × width × 4, float32
    editor.put_pixels(sky[::-1], name="Upside down")

Several calls become one step of history, which one undo takes back, inside
``editor.step(name)``::

    with editor.step("Sky and sea"):
        editor.new_layer(name="Sky")
        editor.new_layer(name="Sea")

What a script changes in an image becomes one history state when it ends (or
disconnects), so that one undo takes back the whole run; in an interactive session each
call is a step of its own.

The connection is the editor's local pipe, found with its token in ``api.json`` in the
user's settings folder; only the user's own programs can read it.
"""

from __future__ import annotations

import base64
import contextlib
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


def _numpy() -> Any:
    """numpy, which pixels come as."""
    try:
        import numpy
    except ImportError:
        raise OpalError("Pixels come as numpy arrays: install numpy (pip install numpy).") from None
    return numpy


#: Whether pixels pass through shared memory (on Windows alone, yet); otherwise in the
#: connection's lines.
_SHARED = sys.platform == "win32"

#: The most pixel bytes sent in one request where they cannot go through shared memory:
#: in base64, with the rest of the request, within the 1 MB a request may be.
_BAND = 700_000

#: The numpy type of each pixel format's values (little-endian, as the editor gives them).
_DTYPES = {"srgb8": "u1", "srgb16": "<u2", "linear": "<f4"}


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
        self._pending = bytearray()

    def exchange(self, message: dict) -> dict:
        """Sends ``message`` and returns the reply to it. Replies to earlier messages still
        on their way (those of a call interrupted while it waited) are passed over."""
        line = (json.dumps(message, default=_plain) + "\n").encode("utf-8")
        try:
            self._send(line)
            while True:
                # Looked for in what came last alone: an answer can be many megabytes.
                looked = 0
                while (end := self._pending.find(b"\n", looked)) < 0:
                    looked = len(self._pending)
                    chunk = self._recv(1 << 20)
                    if not chunk:
                        raise OpalError("The editor closed the connection.")
                    self._pending += chunk
                reply = json.loads(self._pending[:end])
                del self._pending[: end + 1]
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
            self._methods = {m["name"]: m for m in self.call("methods")}
        except BaseException:
            # Not connected after all: nothing is left open.
            self._pipe.close()
            raise
        self.version = hello.get("editor")

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

    @contextlib.contextmanager
    def step(self, name: str):
        """What the calls inside the ``with`` change in each image becomes one state of its
        history, called ``name``, which one undo takes back. ::

            with editor.step("Sky and sea"):
                editor.new_layer(name="Sky")
                editor.new_layer(name="Sea")

        A step inside another is part of it. Outside any step, a script's calls are joined
        into one state when it ends, and an interactive session's are each a state."""
        try:
            self.call("begin_step", name=name)
        except (KeyboardInterrupt, SystemExit):
            # It reached the editor before its answer came (an interrupt while it was
            # awaited): ended, to be sure. (One refused began no step, so ends none.)
            with contextlib.suppress(OpalError, OSError, ValueError):
                self.call("end_step")
            raise
        try:
            yield self
        except BaseException:
            # Ended, if it can be, without hiding what went wrong.
            with contextlib.suppress(OpalError, OSError, ValueError):
                self.call("end_step")
            raise
        self.call("end_step")

    def pixels(
        self,
        document: int | None = None,
        layer: int | None = None,
        x: int = 0,
        y: int = 0,
        width: int | None = None,
        height: int | None = None,
        format: str = "srgb8",
    ) -> Any:
        """The pixels of an image as shown (or of one ``layer`` alone, at full opacity) in a
        rectangle of its canvas (the whole canvas by default; ``x`` and ``y`` from its top
        left): a numpy array of height × width × 4 (red, green, blue, alpha, not
        premultiplied). ``format`` is ``"srgb8"`` (uint8), ``"srgb16"`` (uint16) or
        ``"linear"`` (float32, in linear light). On Windows they come through shared memory,
        without being copied through the connection."""
        np = _numpy()
        given = dict(document=document, layer=layer, width=width, height=height)
        params = {k: v for k, v in given.items() if v is not None}
        got = self.call("pixels", x=x, y=y, format=format, shared=_SHARED, **params)
        dtype = _DTYPES[got["format"]]
        shape = (got["height"], got["width"], 4)
        if "shm" in got:
            import _winapi
            import mmap

            try:
                # Opened as it is, or not at all: by its name alone, mmap would make an empty
                # one where it cannot see the editor's (another Windows session's).
                held = _winapi.OpenFileMapping(_winapi.FILE_MAP_READ, False, got["shm"])
            except OSError as e:
                self.call("release_pixels", shm=got["shm"])
                raise OpalError(f"The pixels' shared memory could not be opened: {e}") from None
            try:
                # Opened before it is released: the editor's goes, this one stays.
                view = mmap.mmap(-1, got["bytes"], tagname=got["shm"])
            finally:
                _winapi.CloseHandle(held)
                self.call("release_pixels", shm=got["shm"])
            return np.frombuffer(view, dtype=dtype).reshape(shape)
        return np.frombuffer(base64.b64decode(got["data"]), dtype=dtype).reshape(shape).copy()

    def put_pixels(
        self,
        pixels: Any,
        document: int | None = None,
        layer: int | None = None,
        x: int = 0,
        y: int = 0,
        name: str | None = None,
    ) -> int:
        """Puts ``pixels`` into an image at ``x``, ``y`` of its canvas: as a new layer above
        the active one (called ``name``), or into the pixels of ``layer``, which they replace
        there. ``pixels`` is height × width × 4 (red, green, blue, alpha, not premultiplied)
        or × 3 (opaque): uint8 or uint16 for sRGB values, floats for linear light. Returns
        the layer's id."""
        np = _numpy()
        a = np.asarray(pixels)
        if a.ndim != 3 or a.shape[2] not in (3, 4):
            raise OpalError(f"Pixels are height × width × 4 (or 3), not {' × '.join(map(str, a.shape))}.")
        if a.shape[0] == 0 or a.shape[1] == 0:
            raise OpalError("There are no pixels to put in: the array is empty.")
        # By kind and size, whatever the byte order (FITS data is big-endian, say).
        if a.dtype.kind == "u" and a.dtype.itemsize == 1:
            format, dtype, opaque = "srgb8", np.uint8, 255
        elif a.dtype.kind == "u" and a.dtype.itemsize == 2:
            format, dtype, opaque = "srgb16", np.uint16, 65535
        elif a.dtype.kind == "f":
            format, dtype, opaque = "linear", np.float32, 1.0
        else:
            raise OpalError(f"Pixels are uint8, uint16 or floats, not {a.dtype}.")
        if a.shape[2] == 3:
            a = np.concatenate([a, np.full(a.shape[:2] + (1,), opaque, dtype=a.dtype)], axis=2)
        a = np.ascontiguousarray(a, dtype=np.dtype(dtype).newbyteorder("<"))
        height, width = a.shape[:2]
        params = dict(width=width, height=height, x=x, y=y, format=format)
        params.update({k: v for k, v in dict(document=document, layer=layer, name=name).items() if v is not None})
        if _SHARED:
            import mmap
            import secrets

            tag = f"Local\\opal-pixels-in-{os.getpid()}-{secrets.token_hex(12)}"
            with mmap.mmap(-1, a.nbytes, tagname=tag) as view:
                target = np.frombuffer(view, dtype=a.dtype).reshape(a.shape)
                target[...] = a
                # The view must go before the memory can be closed.
                del target
                got = self.call("put_pixels", shm=tag, **params)
            return got["layer"]
        # Elsewhere, in the connection's lines, at most 1 MB a request: a band of rows at a
        # time, the first making the layer (if one is to be made), the rest going into it.
        row = width * a.dtype.itemsize * 4
        rows = _BAND // row
        if rows == 0:
            raise OpalError(f"Rows of {width} such pixels are too long to send on this system: send narrower ones.")
        into = layer
        for top in range(0, height, rows):
            band = a[top : top + rows]
            data = base64.b64encode(band.tobytes()).decode("ascii")
            part = dict(params, height=band.shape[0], y=y + top)
            if into is not None:
                part["layer"] = into
                part.pop("name", None)
            into = self.call("put_pixels", data=data, **part)["layer"]
        return into

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
    except (OSError, ValueError) as e:
        raise OpalError(f"{path} is not readable: {e}") from None
    if not isinstance(info, dict) or not isinstance(info.get("pipe"), str) or not isinstance(info.get("token"), str):
        raise OpalError(f"{path} does not name a pipe and a token.")
    return Editor(info["pipe"], info["token"], _running_script() if script is _RUNNING else script)

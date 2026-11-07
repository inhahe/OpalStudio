"""Expressions for the editor to work out, built in Python with operators.

Any number or length a method takes may be an expression (``API.md``, *Expressions*):
text such as ``"Logo.right + 5mm"``, or one built here, which is the same text::

    from opal import layer, canvas, box, fn, mm

    editor.transform(x=box.x + mm(5))                  # 5 mm right of where it is
    editor.place(path="mark.png", width=canvas.width / 4)
    editor.paste(x=layer(logo).right + mm(5), y=fn.mid(layer(a).center, layer(b).center).y)

What is built is only text: the editor reads it and works it out when the call runs,
against the image as it is then, as it does what is typed into its fields, so what it
says is checked there. A bare number is in the call's ``unit`` where it is added to a
length (``box.width + 10``), and a plain factor where it multiplies one.
"""

from __future__ import annotations

import math
import operator
from typing import Union

# How tightly each form binds, as the editor reads it: an atom, a power, a sign, a product,
# a sum.
_ATOM, _POWER, _SIGN, _PRODUCT, _SUM = 5, 4, 3, 2, 1

Operand = Union["Expr", int, float]


class Expr:
    """An expression: its text, as the editor reads it (``str(e)``), built with the
    arithmetic operators, ``abs`` and ``round``, and members (``.right``, ``.center``,
    ``.x``) as the editor's language has them."""

    __slots__ = ("_text", "_binds")

    def __init__(self, text: str, binds: int = _ATOM) -> None:
        self._text = text
        self._binds = binds

    def __str__(self) -> str:
        return self._text

    def __repr__(self) -> str:
        return f"Expr({self._text!r})"

    def __add__(self, other: Operand) -> Expr:
        return _op(self, "+", other)

    def __radd__(self, other: Operand) -> Expr:
        return _op(other, "+", self)

    def __sub__(self, other: Operand) -> Expr:
        return _op(self, "-", other)

    def __rsub__(self, other: Operand) -> Expr:
        return _op(other, "-", self)

    def __mul__(self, other: Operand) -> Expr:
        return _op(self, "*", other)

    def __rmul__(self, other: Operand) -> Expr:
        return _op(other, "*", self)

    def __truediv__(self, other: Operand) -> Expr:
        return _op(self, "/", other)

    def __rtruediv__(self, other: Operand) -> Expr:
        return _op(other, "/", self)

    def __pow__(self, other: Operand) -> Expr:
        return _op(self, "^", other)

    def __rpow__(self, other: Operand) -> Expr:
        return _op(other, "^", self)

    def __neg__(self) -> Expr:
        return Expr("-" + _wrap(self, _SIGN), _SIGN)

    def __pos__(self) -> Expr:
        return self

    def __abs__(self) -> Expr:
        return _call("abs", self)

    def __round__(self, ndigits: None = None) -> Expr:
        if ndigits is not None:
            raise TypeError("An expression rounds to whole pixels, or to a step: fn.round(e, mm(1)).")
        return _call("round", self)

    def __getattr__(self, name: str) -> Expr:
        # Members, as the language has them (a typo is the editor's to tell, as in a field).
        if name.startswith("_"):
            raise AttributeError(name)
        return Expr(f"{_wrap(self, _ATOM)}.{name}")

    def __bool__(self) -> bool:
        raise TypeError("An expression has no truth: the editor works it out when the call runs.")


def _number(value: int | float) -> str:
    """A number as the editor reads it."""
    if isinstance(value, bool):
        raise TypeError("A truth value is not a number.")
    try:
        return str(operator.index(value))
    except TypeError:
        pass
    v = float(value)
    if not math.isfinite(v):
        raise ValueError(f"{v} is not a number an expression may hold.")
    return repr(v)


def _expr(value: Operand) -> Expr:
    """`value` as an expression: a number as a bare one, in the call's unit."""
    if isinstance(value, Expr):
        return value
    if isinstance(value, (int, float)) or hasattr(value, "__index__") or hasattr(value, "__float__"):
        text = _number(value)  # type: ignore[arg-type]
        return Expr(text, _SIGN if text.startswith("-") else _ATOM)
    raise TypeError(f"A {type(value).__name__} cannot be part of an expression.")


def _wrap(e: Expr, need: int) -> str:
    """`e`'s text, in parentheses if it binds less tightly than `need`."""
    return f"({e})" if e._binds < need else str(e)


def _op(a: Operand, op: str, b: Operand) -> Expr:
    left, right = _expr(a), _expr(b)
    binds = {"+": _SUM, "-": _SUM, "*": _PRODUCT, "/": _PRODUCT, "^": _POWER}[op]
    if op == "^":
        # Powers group from the right: a^(b^c) needs none, (a^b)^c does.
        return Expr(f"{_wrap(left, binds + 1)}^{_wrap(right, binds)}", binds)
    # The rest from the left: (a - b) - c needs none, a - (b - c) does.
    return Expr(f"{_wrap(left, binds)} {op} {_wrap(right, binds + 1)}", binds)


def _call(name: str, *args: Operand) -> Expr:
    return Expr(f"{name}({', '.join(str(_expr(a)) for a in args)})")


def layer(which: int | str) -> Expr:
    """A layer, by its id (as the editor's methods give it) or its name: its bounds, whose
    members are ``left``, ``right``, ``top``, ``bottom``, ``width``, ``height``, ``x``,
    ``y`` and the points ``top_left``, ``top_right``, ``bottom_left``, ``bottom_right`` and
    ``center`` (each with ``.x`` and ``.y``). By id it stays the same layer however it is
    renamed."""
    if isinstance(which, bool):
        raise TypeError("A layer is named by its id or its name.")
    if isinstance(which, int):
        if which < 0:
            raise ValueError("A layer's id is not below 0.")
        return Expr(f"layer({which})")
    if isinstance(which, str):
        escaped = which.replace("\\", "\\\\").replace('"', '\\"')
        return Expr(f'layer("{escaped}")')
    raise TypeError("A layer is named by its id or its name.")


#: The canvas's bounds (members as a layer's).
canvas = Expr("canvas")
#: The image's resolution, in pixels per inch: a plain number.
ppi = Expr("ppi")
#: π.
pi = Expr("pi")


class _Box:
    """The box a call places, as it was before the call: its position (its anchor point's,
    from what it counts from), size and angle."""

    x = Expr("x")
    y = Expr("y")
    width = Expr("width")
    height = Expr("height")
    angle = Expr("angle")


#: The box a call places (``box.x``, ``box.width``), as it was before the call.
box = _Box()


def _in(unit: str):
    def length(value: Operand) -> Expr:
        if isinstance(value, Expr):
            return value * Expr(f"1{unit}")
        text = _number(value)
        return Expr(f"{text}{unit}", _SIGN if text.startswith("-") else _ATOM)

    length.__doc__ = (
        f"`value` in {unit}: a number, or an expression that comes to a plain number "
        f"(`{unit}(box.width / box.height)`), times one {unit}: of a length, it would be an area."
    )
    length.__name__ = {"in": "inch"}.get(unit, unit)
    return length


px = _in("px")
pt = _in("pt")
pc = _in("pc")
inch = _in("in")
ft = _in("ft")
yd = _in("yd")
mm = _in("mm")
cm = _in("cm")
m = _in("m")
deg = _in("deg")
rad = _in("rad")


class _Functions:
    """The language's functions."""

    @staticmethod
    def min(*values: Operand) -> Expr:
        return _call("min", *values)

    @staticmethod
    def max(*values: Operand) -> Expr:
        return _call("max", *values)

    @staticmethod
    def abs(value: Operand) -> Expr:
        return _call("abs", value)

    @staticmethod
    def round(value: Operand, step: Operand | None = None) -> Expr:
        """To whole pixels (an angle to whole degrees), or to a multiple of `step`."""
        return _call("round", value) if step is None else _call("round", value, step)

    @staticmethod
    def floor(value: Operand, step: Operand | None = None) -> Expr:
        return _call("floor", value) if step is None else _call("floor", value, step)

    @staticmethod
    def ceil(value: Operand, step: Operand | None = None) -> Expr:
        return _call("ceil", value) if step is None else _call("ceil", value, step)

    @staticmethod
    def sqrt(value: Operand) -> Expr:
        return _call("sqrt", value)

    @staticmethod
    def sin(angle: Operand) -> Expr:
        return _call("sin", angle)

    @staticmethod
    def cos(angle: Operand) -> Expr:
        return _call("cos", angle)

    @staticmethod
    def tan(angle: Operand) -> Expr:
        return _call("tan", angle)

    @staticmethod
    def asin(value: Operand) -> Expr:
        return _call("asin", value)

    @staticmethod
    def acos(value: Operand) -> Expr:
        return _call("acos", value)

    @staticmethod
    def atan(value: Operand) -> Expr:
        return _call("atan", value)

    @staticmethod
    def atan2(y: Operand, x: Operand) -> Expr:
        return _call("atan2", y, x)

    @staticmethod
    def mid(a: Expr, b: Expr) -> Expr:
        """The point halfway between points `a` and `b`."""
        return _call("mid", a, b)

    @staticmethod
    def lerp(a: Operand, b: Operand, t: Operand) -> Expr:
        """`t` of the way from `a` to `b`: points, or lengths."""
        return _call("lerp", a, b, t)

    @staticmethod
    def dist(a: Expr, b: Expr) -> Expr:
        """How far apart points `a` and `b` are."""
        return _call("dist", a, b)

    @staticmethod
    def point(x: Operand, y: Operand) -> Expr:
        """The point at `x`, `y`."""
        return _call("point", x, y)


#: The language's functions: ``fn.min``, ``fn.round``, ``fn.mid`` and the rest.
fn = _Functions()

__all__ = [
    "Expr",
    "layer",
    "canvas",
    "ppi",
    "pi",
    "box",
    "fn",
    "px",
    "pt",
    "pc",
    "inch",
    "ft",
    "yd",
    "mm",
    "cm",
    "m",
    "deg",
    "rad",
]

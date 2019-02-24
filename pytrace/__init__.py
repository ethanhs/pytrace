from contextlib import contextmanager
from pytrace_native import hook, unhook


@contextmanager
def trace_interpreter():
    hook()
    try:
        yield
    finally:
        unhook()

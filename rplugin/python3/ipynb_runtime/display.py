"""numpy's native book-style scalar display, without changing stored values."""

# run in an isolated namespace so the notebook gains no helper variables.
NUMPY_STARTUP = """exec('''try:
    import numpy
except ImportError:
    pass
else:
    if int(numpy.__version__.split('.')[0]) >= 2:
        numpy.set_printoptions(legacy='1.25')
''', {})"""

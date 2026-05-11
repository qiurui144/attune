"""Empty conftest to shadow the parent tests/conftest.py which imports
npu_webhook (legacy Python prototype) modules that may not be installed.
F-15-MCP tests only need stdlib + pytest."""

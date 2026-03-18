# -*- mode: python ; coding: utf-8 -*-
"""PyInstaller 打包配置"""

a = Analysis(
    ['../src/npu_webhook/main.py'],
    pathex=[],
    binaries=[],
    datas=[
        ('../extension/dist', 'extension'),
    ],
    hiddenimports=[
        'chromadb',
        'chromadb.api',
        'chromadb.config',
        'uvicorn',
        'uvicorn.logging',
        'uvicorn.protocols',
        'uvicorn.protocols.http',
        'uvicorn.protocols.http.auto',
        'uvicorn.protocols.websockets',
        'uvicorn.protocols.websockets.auto',
        'uvicorn.lifespan',
        'uvicorn.lifespan.on',
    ],
    module_collection_mode={
        'chromadb': 'py',
        'onnxruntime': 'py',
    },
    noarchive=False,
)

pyz = PYZ(a.pure)

exe = EXE(
    pyz,
    a.scripts,
    [],
    exclude_binaries=True,
    name='npu-webhook',
    debug=False,
    strip=False,
    upx=True,
    console=False,
)

coll = COLLECT(
    exe,
    a.binaries,
    a.datas,
    strip=False,
    upx=True,
    name='npu-webhook',
)

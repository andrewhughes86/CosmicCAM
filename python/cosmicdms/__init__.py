"""Python API for CosmicDMS.

Start the app and connect::

    import cosmicdms
    with cosmicdms.launch(sample=True) as cc:
        cc.view_theme_set(theme="light")

or connect to an app already started with ``--control PORT``::

    cc = cosmicdms.connect(PORT)
"""

from .client import CosmicDMS, CosmicDMSError, connect, launch

__all__ = ["CosmicDMS", "CosmicDMSError", "connect", "launch"]
__version__ = "0.1.0"

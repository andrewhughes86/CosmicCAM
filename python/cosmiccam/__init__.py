"""Python API for CosmicCAM.

Start the app and connect::

    import cosmiccam
    with cosmiccam.launch(sample=True) as cc:
        cc.view_theme_set(theme="light")

or connect to an app already started with ``--control PORT``::

    cc = cosmiccam.connect(PORT)
"""

from .client import CosmicCAM, CosmicCAMError, connect, launch

__all__ = ["CosmicCAM", "CosmicCAMError", "connect", "launch"]
__version__ = "0.1.0"

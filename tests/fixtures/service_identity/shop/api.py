from .cart import Cart
from .catalog import Catalog
from .checkout import checkout
from .storage import load


def handle(catalog: Catalog, request: dict) -> dict:
    cart = Cart(catalog)
    for sku, quantity in request.get("items", []):
        cart.add(sku, quantity)
    receipt = checkout(cart, request.get("card", ""))
    return {"order": load(receipt).number}

from .cart import Cart
from .orders import open_order
from .payments import charge
from .storage import dump


def checkout(cart: Cart, card: str) -> str:
    order = open_order(cart.items)
    charge(order, cart.total(), card)
    return dump(order)

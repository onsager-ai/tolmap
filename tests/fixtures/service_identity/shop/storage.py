import json

from .models import Order
from .orders import open_order


def dump(order: Order) -> str:
    return json.dumps({"number": order.number, "paid": order.paid})


def load(text: str) -> Order:
    data = json.loads(text)
    order = open_order([])
    order.paid = data["paid"]
    return order

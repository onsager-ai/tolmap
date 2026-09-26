from itertools import count

from .models import Order

_numbers = count(1)


def open_order(items) -> Order:
    return Order(number=next(_numbers), items=list(items))


def mark_paid(order: Order) -> Order:
    order.paid = True
    return order

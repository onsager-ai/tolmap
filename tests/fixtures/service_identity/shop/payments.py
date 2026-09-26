from .orders import mark_paid


class PaymentDeclined(Exception):
    pass


def charge(order, amount_cents: int, card: str):
    if not card or amount_cents <= 0:
        raise PaymentDeclined(order.number)
    return mark_paid(order)

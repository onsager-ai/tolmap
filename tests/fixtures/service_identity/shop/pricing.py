from .models import LineItem
from .tax import tax_cents


def line_total(item: LineItem) -> int:
    subtotal = item.subtotal()
    return subtotal + tax_cents(subtotal)


def discount(total_cents: int, percent: int) -> int:
    return total_cents - total_cents * percent // 100

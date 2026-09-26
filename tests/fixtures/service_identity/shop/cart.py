from .catalog import Catalog
from .models import LineItem
from .pricing import line_total


class Cart:
    def __init__(self, catalog: Catalog):
        self.catalog = catalog
        self.items = []

    def add(self, sku: str, quantity: int = 1) -> None:
        self.items.append(LineItem(self.catalog.find(sku), quantity))

    def total(self) -> int:
        return sum(line_total(item) for item in self.items)

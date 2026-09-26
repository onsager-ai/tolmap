from .models import Product
from .pricing import discount


class Catalog:
    def __init__(self):
        self._products = {}

    def add(self, product: Product) -> None:
        self._products[product.sku] = product

    def find(self, sku: str) -> Product:
        return self._products[sku]

    def sale_price(self, sku: str, percent: int) -> int:
        return discount(self.find(sku).price_cents, percent)

from dataclasses import dataclass, field


@dataclass
class Product:
    sku: str
    name: str
    price_cents: int


@dataclass
class LineItem:
    product: Product
    quantity: int = 1

    def subtotal(self) -> int:
        return self.product.price_cents * self.quantity


@dataclass
class Order:
    number: int
    items: list = field(default_factory=list)
    paid: bool = False

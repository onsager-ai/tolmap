import sys

from .api import handle
from .catalog import Catalog
from .models import Product


def main(argv=None) -> int:
    catalog = Catalog()
    catalog.add(Product("tea", "Tea", 450))
    catalog.add(Product("mug", "Mug", 1200))
    print(handle(catalog, {"items": [("tea", 2), ("mug", 1)], "card": "4242"}))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

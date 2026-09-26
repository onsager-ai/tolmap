RATES = {"standard": 0.2, "reduced": 0.05, "zero": 0.0}


def rate_for(category: str) -> float:
    return RATES.get(category, RATES["standard"])


def tax_cents(amount_cents: int, category: str = "standard") -> int:
    return round(amount_cents * rate_for(category))

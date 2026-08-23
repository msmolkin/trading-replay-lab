"""Explicit ingestion request/byte/cost budget enforcement."""

from __future__ import annotations

from dataclasses import dataclass


class BudgetExceeded(RuntimeError):
    """Raised before an ingestion action would exceed its declared budget."""


@dataclass(slots=True)
class BudgetGuard:
    """Mutable accounting guard; correctness never depends on provider throttling."""

    max_requests: int
    max_bytes: int
    max_cost_minor: int
    requests: int = 0
    bytes: int = 0
    cost_minor: int = 0

    def preflight(self, *, requests: int, cost_minor: int) -> None:
        """Reject a plan whose declared request/cost envelope already exceeds limits."""
        if requests < 0 or cost_minor < 0:
            raise ValueError("budget reservations cannot be negative")
        if self.requests + requests > self.max_requests:
            raise BudgetExceeded("request budget exceeded")
        if self.cost_minor + cost_minor > self.max_cost_minor:
            raise BudgetExceeded("cost budget exceeded")

    def consume(self, *, bytes_count: int, cost_minor: int) -> None:
        """Account for one completed network request.

        Raises before counters mutate if the actual response exceeds a bound.
        """
        if bytes_count < 0 or cost_minor < 0:
            raise ValueError("budget consumption cannot be negative")
        next_requests = self.requests + 1
        next_bytes = self.bytes + bytes_count
        next_cost = self.cost_minor + cost_minor
        if next_requests > self.max_requests:
            raise BudgetExceeded("request budget exceeded")
        if next_bytes > self.max_bytes:
            raise BudgetExceeded("byte budget exceeded")
        if next_cost > self.max_cost_minor:
            raise BudgetExceeded("cost budget exceeded")
        self.requests = next_requests
        self.bytes = next_bytes
        self.cost_minor = next_cost

// Package practice implements the deterministic, self-contained trading
// simulator used by the first playable practice run.
//
// Prices are integer cents, quantities are integer milli-units, and rates are
// integer parts per billion. A Session deliberately keeps its future price path
// in unexported state; callers can serialize Snapshot values, which contain only
// prices at or before the current reveal cursor.
package practice

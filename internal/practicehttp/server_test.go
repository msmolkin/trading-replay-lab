package practicehttp

import (
	"bytes"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/msmolkin/trading-replay-lab/internal/practice"
)

func TestPlayableHTTPJourneyUsesGoCore(t *testing.T) {
	server := httptest.NewServer(New().Handler())
	defer server.Close()

	state := post[practice.Snapshot](t, server.URL+"/v1/practice", `{}`, http.StatusCreated)
	if len(state.Revealed) != 1 {
		t.Fatalf("start revealed %d prices, want exactly one", len(state.Revealed))
	}
	state = post[practice.Snapshot](t, server.URL+"/v1/practice/"+state.SessionID+"/order", `{"intent":"BUY","quantityMilli":100,"priceMode":"MARKET","reduceOnly":false,"postOnly":false,"marketableOnly":true}`, http.StatusOK)
	if state.PositionMilli != 100 {
		t.Fatalf("buy position = %d, want 100", state.PositionMilli)
	}
	state = post[practice.Snapshot](t, server.URL+"/v1/practice/"+state.SessionID+"/leverage", `{"leverage":10}`, http.StatusOK)
	if state.Leverage != 10 {
		t.Fatalf("leverage = %d, want 10", state.Leverage)
	}
	state = post[practice.Snapshot](t, server.URL+"/v1/practice/"+state.SessionID+"/order", `{"intent":"SELL","quantityMilli":250,"priceMode":"MARKET","reduceOnly":false,"postOnly":false,"marketableOnly":true}`, http.StatusOK)
	if state.PositionMilli != -150 {
		t.Fatalf("sell-through-zero position = %d, want -150", state.PositionMilli)
	}
}

func TestRejectsUnknownJSONFields(t *testing.T) {
	server := httptest.NewServer(New().Handler())
	defer server.Close()
	state := post[practice.Snapshot](t, server.URL+"/v1/practice", `{}`, http.StatusCreated)
	response := post[map[string]string](t, server.URL+"/v1/practice/"+state.SessionID+"/leverage", `{"leverage":10,"futureLeak":true}`, http.StatusBadRequest)
	if response["code"] != "INVALID_REQUEST" || !strings.Contains(response["error"], "unknown field") {
		t.Fatalf("response = %#v, want strict JSON rejection", response)
	}
}

func post[T any](t *testing.T, url, body string, wantStatus int) T {
	t.Helper()
	response, err := http.Post(url, "application/json", bytes.NewBufferString(body))
	if err != nil {
		t.Fatalf("POST %s: %v", url, err)
	}
	defer response.Body.Close()
	if response.StatusCode != wantStatus {
		t.Fatalf("POST %s status = %d, want %d", url, response.StatusCode, wantStatus)
	}
	var value T
	if err := json.NewDecoder(response.Body).Decode(&value); err != nil {
		t.Fatalf("decode POST %s: %v", url, err)
	}
	return value
}

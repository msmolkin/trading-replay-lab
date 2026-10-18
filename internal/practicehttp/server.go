package practicehttp

import (
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"strings"
	"sync"

	"github.com/msmolkin/trading-replay-lab/internal/practice"
)

const maxRequestBytes = 1 << 20

// Server exposes the practice simulator over a small same-host JSON API.
// A single mutex makes each command atomic and preserves deterministic order.
type Server struct {
	mu       sync.Mutex
	sessions map[string]*practice.Session
}

func New() *Server {
	return &Server{sessions: make(map[string]*practice.Session)}
}

func (s *Server) Handler() http.Handler {
	mux := http.NewServeMux()
	mux.HandleFunc("POST /v1/practice", s.start)
	mux.HandleFunc("POST /v1/practice/{sessionID}/{action}", s.command)
	mux.HandleFunc("GET /healthz", func(w http.ResponseWriter, _ *http.Request) {
		writeJSON(w, http.StatusOK, map[string]string{"status": "ok"})
	})
	return mux
}

func (s *Server) start(w http.ResponseWriter, _ *http.Request) {
	id, err := sessionID()
	if err != nil {
		writeError(w, http.StatusInternalServerError, "SESSION_ID_FAILED", "could not create a practice session")
		return
	}
	s.mu.Lock()
	session := practice.NewSession(id)
	s.sessions[id] = session
	state := session.Snapshot()
	s.mu.Unlock()
	writeJSON(w, http.StatusCreated, state)
}

func (s *Server) command(w http.ResponseWriter, r *http.Request) {
	sessionID := r.PathValue("sessionID")
	action := r.PathValue("action")
	if sessionID == "" || strings.Contains(sessionID, "/") {
		writeError(w, http.StatusBadRequest, "INVALID_SESSION", "invalid practice session")
		return
	}

	var advanceRequest struct {
		Steps int `json:"steps"`
	}
	var orderRequest practice.OrderRequest
	var leverageRequest struct {
		Leverage int `json:"leverage"`
	}
	switch action {
	case "advance":
		if err := decodeJSON(w, r, &advanceRequest); err != nil {
			writeError(w, http.StatusBadRequest, "INVALID_REQUEST", err.Error())
			return
		}
		if advanceRequest.Steps == 0 {
			advanceRequest.Steps = 1
		}
	case "order":
		if err := decodeJSON(w, r, &orderRequest); err != nil {
			writeError(w, http.StatusBadRequest, "INVALID_REQUEST", err.Error())
			return
		}
	case "leverage":
		if err := decodeJSON(w, r, &leverageRequest); err != nil {
			writeError(w, http.StatusBadRequest, "INVALID_REQUEST", err.Error())
			return
		}
	default:
		writeError(w, http.StatusNotFound, "UNKNOWN_ACTION", "unknown practice command")
		return
	}

	s.mu.Lock()
	defer s.mu.Unlock()
	session := s.sessions[sessionID]
	if session == nil {
		writeError(w, http.StatusNotFound, "SESSION_NOT_FOUND", "practice session was not found; start a new run")
		return
	}

	var (
		state practice.Snapshot
		err   error
	)
	switch action {
	case "advance":
		state, err = session.Advance(advanceRequest.Steps)
	case "order":
		state, err = session.SubmitOrder(orderRequest)
	case "leverage":
		state, err = session.SetLeverage(leverageRequest.Leverage)
	}
	if err != nil {
		var domainError *practice.Error
		if errors.As(err, &domainError) {
			writeError(w, domainError.HTTPStatus, domainError.Code, domainError.Message)
			return
		}
		writeError(w, http.StatusBadRequest, "INVALID_REQUEST", err.Error())
		return
	}
	writeJSON(w, http.StatusOK, state)
}

func decodeJSON(w http.ResponseWriter, r *http.Request, destination any) error {
	r.Body = http.MaxBytesReader(w, r.Body, maxRequestBytes)
	decoder := json.NewDecoder(r.Body)
	decoder.DisallowUnknownFields()
	if err := decoder.Decode(destination); err != nil {
		return fmt.Errorf("invalid JSON request: %w", err)
	}
	if err := decoder.Decode(&struct{}{}); !errors.Is(err, io.EOF) {
		return errors.New("invalid JSON request: expected one object")
	}
	return nil
}

func sessionID() (string, error) {
	bytes := make([]byte, 16)
	if _, err := rand.Read(bytes); err != nil {
		return "", err
	}
	return hex.EncodeToString(bytes), nil
}

func writeError(w http.ResponseWriter, status int, code, message string) {
	writeJSON(w, status, map[string]string{"code": code, "error": message})
}

func writeJSON(w http.ResponseWriter, status int, value any) {
	w.Header().Set("Content-Type", "application/json; charset=utf-8")
	w.Header().Set("Cache-Control", "no-store")
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(value)
}

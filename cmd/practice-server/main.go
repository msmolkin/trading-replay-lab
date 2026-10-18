package main

import (
	"errors"
	"log"
	"net/http"
	"os"
	"time"

	"github.com/msmolkin/trading-replay-lab/internal/practicehttp"
)

func main() {
	address := os.Getenv("PRACTICE_SERVER_ADDR")
	if address == "" {
		address = "127.0.0.1:8787"
	}
	server := &http.Server{
		Addr:              address,
		Handler:           practicehttp.New().Handler(),
		ReadHeaderTimeout: 5 * time.Second,
		ReadTimeout:       10 * time.Second,
		WriteTimeout:      10 * time.Second,
		IdleTimeout:       60 * time.Second,
	}
	log.Printf("practice server listening on http://%s", address)
	if err := server.ListenAndServe(); err != nil && !errors.Is(err, http.ErrServerClosed) {
		log.Fatal(err)
	}
}

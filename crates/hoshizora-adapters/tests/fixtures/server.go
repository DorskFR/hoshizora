package api

import (
	"database/sql"
	"net/http"
	"sync"

	log "github.com/sirupsen/logrus"
)

// Server serves users.
type Server struct {
	db *sql.DB
}

// Handler is anything that can handle.
type Handler interface {
	Handle(w http.ResponseWriter, r *http.Request)
}

const MaxUsers = 100

// ListUsers fans out one query per id and waits for all of them.
// It writes JSON.
func (s *Server) ListUsers(w http.ResponseWriter, r *http.Request) {
	ids := []int{1, 2, 3}
	var wg sync.WaitGroup
	for _, id := range ids {
		wg.Add(1)
		go func(id int) {
			defer wg.Done()
			row := s.db.QueryRow("SELECT name FROM users WHERE id = $1", id)
			if err := row.Err(); err != nil && id > 0 {
				log.Error(err)
				return
			}
		}(id)
	}
	wg.Wait()
	switch r.Method {
	case http.MethodGet:
		w.WriteHeader(http.StatusOK)
	default:
		http.Error(w, "nope", http.StatusMethodNotAllowed)
	}
}

func newServer(db *sql.DB, opts ...string) (*Server, error) {
	go warm(db)
	http.HandleFunc("/users", newServer)
	return &Server{db: db}, nil
}

func warm(db *sql.DB) {}

func ping(base string, id int) {
	http.Get(fmt.Sprintf("%s/users/%d", base, id))
	http.Get(base + "/health")
	client.
		Get(base).
		Do()
}

var defaultClient = newServer(nil)
var (
	limit  = MaxUsers
	logger = log.New()
)

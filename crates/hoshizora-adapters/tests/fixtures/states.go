package states

type Status string

const (
	Active Status = "active"
	Paused Status = "paused"
	Done   Status = "done"
)

type Kind int

const (
	KindA Kind = iota
	KindB
)

func label(s Status) string {
	switch s {
	case Active, Paused:
		return "a"
	default:
		return ""
	}
}

func live(s Status) bool {
	if s == Active || s == Paused {
		return true
	} else if s == "done" {
		return false
	} else {
		panic("bad")
	}
}

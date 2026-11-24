package somtumpp

import (
	"os"
	"testing"
)

func load(t testing.TB) *Map {
	b, err := os.ReadFile("../../tests/maps/2964220.osu")
	if err != nil {
		t.Fatal(err)
	}
	m, err := ParseMap(b)
	if err != nil {
		t.Fatal(err)
	}
	return m
}

func TestRoutes(t *testing.T) {
	m := load(t)
	defer m.Free()
	if m.NObjects() != 306 || m.Mode() != 0 {
		t.Fatalf("objects %d mode %d", m.NObjects(), m.Mode())
	}
	want := map[uint32]Calculator{0: Official, Relax: Relax2019, Autopilot: Akatsuki}
	for mods, c := range want {
		s := NewScore(0, mods)
		s.Accuracy = 100
		p, err := m.Calculate(s)
		if err != nil || p.Calculator != c || p.PP < 10 || p.Stars < 1 {
			t.Fatalf("mods %d: %+v %v", mods, p, err)
		}
	}
	// Same number the Rust tests and the sidecar parity run see for an RX SS on this map.
	s := NewScore(0, Relax)
	s.Accuracy = 100
	p, _ := m.Calculate(s)
	if p.PP < 151.5 || p.PP > 151.6 {
		t.Fatalf("rx ss pp %v", p.PP)
	}
	st, err := m.Stats(NewScore(0, 64))
	if err != nil || st.ClockRate != 1.5 {
		t.Fatalf("stats %+v %v", st, err)
	}
	v, sec, err := m.Strains(NewScore(0, 0))
	if err != nil || len(v) < 10 || sec <= 0 {
		t.Fatalf("strains %d %v %v", len(v), sec, err)
	}
	if Route(0, Relax) != Relax2019 || Version() == "" {
		t.Fatal("route/version")
	}
}

func TestGarbage(t *testing.T) {
	if _, err := ParseMap(nil); err == nil {
		t.Fatal("nil map parsed")
	}
}

func BenchmarkLiveRelax(b *testing.B) {
	m := load(b)
	defer m.Free()
	n := m.NObjects()
	for i := 0; i < b.N; i++ {
		s := NewScore(0, Relax)
		s.PassedObjects, s.N300, s.MaxCombo = i%n+1, i%n+1, i%n+1
		m.Calculate(s)
	}
}

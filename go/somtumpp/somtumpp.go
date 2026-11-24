// Package somtumpp is osu!somtum pp (vanilla, Relax, Autopilot) as the somtum server pays it.
//
// It runs the somtum-pp WebAssembly build (embedded) on wazero, so it is pure Go: no cgo, no Rust
// toolchain, and it cross-compiles to every GOOS/GOARCH as is. Safe for concurrent use (calls are
// serialised; one calculation is well under a millisecond on a typical map).
package somtumpp

import (
	"context"
	_ "embed"
	"encoding/binary"
	"errors"
	"math"
	"runtime"
	"sync"

	"github.com/tetratelabs/wazero"
	"github.com/tetratelabs/wazero/api"
)

//go:embed somtum_pp.wasm
var wasmBytes []byte

// Legacy mod bits the routing looks at.
const (
	Relax     = 128
	Autopilot = 8192
)

// Calculator that pays a play.
type Calculator int

const (
	Official  Calculator = 0 // ppy's pp (vanilla)
	Relax2019 Calculator = 1 // akatsuki-pp-rs osu_2019 (osu! Relax)
	Akatsuki  Calculator = 2 // akatsuki-pp-rs main (Autopilot; Relax on taiko/catch/mania)
	Failed    Calculator = -1
)

func (c Calculator) String() string {
	switch c {
	case Official:
		return "official"
	case Relax2019:
		return "relax"
	case Akatsuki:
		return "akatsuki"
	}
	return "none"
}

// Score is one play, in osu!stable's terms. Use NewScore for the defaults (whole map, full
// combo, hit counts as given).
type Score struct {
	Mode             int     // vanilla ruleset 0-3
	Mods             uint32  // legacy bits
	Lazer            bool    // made on lazer (TumThai)
	ClockRate        float64 // lazer rate mod speed; <= 0 = the mod's usual one
	MaxCombo         int     // < 0 = full combo
	N300, N100, N50  int
	NGeki, NKatu     int
	NMiss            int
	LargeTickHits    int     // < 0 = unknown (lazer only)
	SliderEndHits    int     // < 0 = unknown (lazer only)
	PassedObjects    int     // < 0 = whole map
	LegacyTotalScore int     // <= 0 = unknown
	Accuracy         float64 // 0-100: generate hits from it; < 0 = use the N* counts
}

// NewScore is a whole-map full-combo score with no hits set; set Accuracy or the counts.
func NewScore(mode int, mods uint32) Score {
	return Score{Mode: mode, Mods: mods, ClockRate: -1, MaxCombo: -1, LargeTickHits: -1, SliderEndHits: -1, PassedObjects: -1, LegacyTotalScore: -1, Accuracy: -1}
}

func (s *Score) fields() [16]float64 {
	b := 0.0
	if s.Lazer {
		b = 1
	}
	return [16]float64{float64(s.Mode), float64(s.Mods), b, s.ClockRate, float64(s.MaxCombo),
		float64(s.N300), float64(s.N100), float64(s.N50), float64(s.NGeki), float64(s.NKatu), float64(s.NMiss),
		float64(s.LargeTickHits), float64(s.SliderEndHits), float64(s.PassedObjects), float64(s.LegacyTotalScore), s.Accuracy}
}

// Perf is pp and stars for a score.
type Perf struct {
	PP, PPAim, PPSpeed, PPAcc, PPFlashlight float64
	Stars                                   float64
	MaxCombo                                int
	EffectiveMissCount                      float64
	Calculator                              Calculator
}

// Stats are AR/OD/CS/HP with a play's mods and speed applied.
type Stats struct {
	AR, OD, CS, HP, ClockRate float64
	NObjects, Mode            int
}

type engine struct {
	mu                        sync.Mutex
	mod                       api.Module
	mem                       api.Memory
	alloc, freeBytes          api.Function
	mapNew, mapFree, mapMode  api.Function
	mapNObjects, calc, stats  api.Function
	strains, route, versionFn api.Function
	scorePtr, outPtr, secPtr  uint32
}

var (
	once    sync.Once
	eng     *engine
	initErr error
	ctx     = context.Background()
)

func get() (*engine, error) {
	once.Do(func() {
		rt := wazero.NewRuntime(ctx)
		mod, err := rt.Instantiate(ctx, wasmBytes)
		if err != nil {
			initErr = err
			return
		}
		e := &engine{mod: mod, mem: mod.Memory()}
		f := mod.ExportedFunction
		e.alloc, e.freeBytes = f("spp_alloc"), f("spp_free_bytes")
		e.mapNew, e.mapFree, e.mapMode, e.mapNObjects = f("spp_map_new"), f("spp_map_free"), f("spp_map_mode"), f("spp_map_n_objects")
		e.calc, e.stats, e.strains, e.route, e.versionFn = f("spp_calculate"), f("spp_stats"), f("spp_strains"), f("spp_route"), f("spp_version")
		if r, err := f("spp_abi_score_fields").Call(ctx); err != nil || r[0] != 16 {
			initErr = errors.New("somtumpp: wasm ABI mismatch")
			return
		}
		e.scorePtr = e.mustAlloc(16 * 8)
		e.outPtr = e.mustAlloc(16 * 8)
		e.secPtr = e.mustAlloc(8)
		eng = e
	})
	return eng, initErr
}

func (e *engine) mustAlloc(n uint32) uint32 {
	r, err := e.alloc.Call(ctx, uint64(n))
	if err != nil {
		panic(err)
	}
	return uint32(r[0])
}

func (e *engine) writeF64s(ptr uint32, v []float64) {
	buf := make([]byte, 8*len(v))
	for i, x := range v {
		binary.LittleEndian.PutUint64(buf[i*8:], math.Float64bits(x))
	}
	e.mem.Write(ptr, buf)
}

func (e *engine) readF64s(ptr uint32, n int) []float64 {
	buf, _ := e.mem.Read(ptr, uint32(8*n))
	out := make([]float64, n)
	for i := range out {
		out[i] = math.Float64frombits(binary.LittleEndian.Uint64(buf[i*8:]))
	}
	return out
}

// Map is a parsed beatmap. Free it when done (a finalizer frees it otherwise).
type Map struct {
	ptr uint32
}

var ErrParse = errors.New("somtumpp: beatmap could not be parsed or is too suspicious to calculate")

// ParseMap parses .osu bytes.
func ParseMap(osu []byte) (*Map, error) {
	e, err := get()
	if err != nil {
		return nil, err
	}
	if len(osu) == 0 {
		return nil, ErrParse
	}
	e.mu.Lock()
	defer e.mu.Unlock()
	p := e.mustAlloc(uint32(len(osu)))
	e.mem.Write(p, osu)
	r, err := e.mapNew.Call(ctx, uint64(p), uint64(len(osu)))
	e.freeBytes.Call(ctx, uint64(p), uint64(len(osu)))
	if err != nil || r[0] == 0 {
		return nil, ErrParse
	}
	m := &Map{ptr: uint32(r[0])}
	runtime.SetFinalizer(m, (*Map).Free)
	return m, nil
}

// Free releases the map. Safe to call twice.
func (m *Map) Free() {
	if m == nil || m.ptr == 0 {
		return
	}
	e, _ := get()
	e.mu.Lock()
	e.mapFree.Call(ctx, uint64(m.ptr))
	e.mu.Unlock()
	m.ptr = 0
	runtime.SetFinalizer(m, nil)
}

// Mode is the map's own mode (0-3).
func (m *Map) Mode() int {
	e, _ := get()
	e.mu.Lock()
	defer e.mu.Unlock()
	r, _ := e.mapMode.Call(ctx, uint64(m.ptr))
	return int(int32(r[0]))
}

// NObjects is the number of hit objects.
func (m *Map) NObjects() int {
	e, _ := get()
	e.mu.Lock()
	defer e.mu.Unlock()
	r, _ := e.mapNObjects.Call(ctx, uint64(m.ptr))
	return int(uint32(r[0]))
}

var ErrCalc = errors.New("somtumpp: calculation failed")

// Calculate is pp as the somtum server pays it.
func (m *Map) Calculate(s Score) (Perf, error) {
	if m == nil || m.ptr == 0 {
		return Perf{Calculator: Failed}, ErrCalc
	}
	e, _ := get()
	e.mu.Lock()
	defer e.mu.Unlock()
	f := s.fields()
	e.writeF64s(e.scorePtr, f[:])
	r, err := e.calc.Call(ctx, uint64(m.ptr), uint64(e.scorePtr), uint64(e.outPtr))
	if err != nil || int32(r[0]) != 0 {
		return Perf{Calculator: Failed}, ErrCalc
	}
	o := e.readF64s(e.outPtr, 9)
	return Perf{PP: o[0], PPAim: o[1], PPSpeed: o[2], PPAcc: o[3], PPFlashlight: o[4], Stars: o[5],
		MaxCombo: int(o[6]), EffectiveMissCount: o[7], Calculator: Calculator(int(o[8]))}, nil
}

// Stats are the map's AR/OD/CS/HP with the score's mods and speed (only Mode, Mods, Lazer,
// ClockRate are read).
func (m *Map) Stats(s Score) (Stats, error) {
	if m == nil || m.ptr == 0 {
		return Stats{}, ErrCalc
	}
	e, _ := get()
	e.mu.Lock()
	defer e.mu.Unlock()
	f := s.fields()
	e.writeF64s(e.scorePtr, f[:])
	r, err := e.stats.Call(ctx, uint64(m.ptr), uint64(e.scorePtr), uint64(e.outPtr))
	if err != nil || int32(r[0]) != 0 {
		return Stats{}, ErrCalc
	}
	o := e.readF64s(e.outPtr, 7)
	return Stats{AR: o[0], OD: o[1], CS: o[2], HP: o[3], ClockRate: o[4], NObjects: int(o[5]), Mode: int(o[6])}, nil
}

// Strains is the difficulty graph: strain peaks per section of sectionLenMs.
func (m *Map) Strains(s Score) (values []float64, sectionLenMs float64, err error) {
	if m == nil || m.ptr == 0 {
		return nil, 0, ErrCalc
	}
	e, _ := get()
	e.mu.Lock()
	defer e.mu.Unlock()
	f := s.fields()
	e.writeF64s(e.scorePtr, f[:])
	r, err := e.strains.Call(ctx, uint64(m.ptr), uint64(e.scorePtr), 0, 0, uint64(e.secPtr))
	if err != nil || int64(r[0]) < 0 {
		return nil, 0, ErrCalc
	}
	n := uint32(r[0])
	if n == 0 {
		return nil, e.readF64s(e.secPtr, 1)[0], nil
	}
	buf := e.mustAlloc(n * 8)
	defer e.freeBytes.Call(ctx, uint64(buf), uint64(n*8))
	e.writeF64s(e.scorePtr, f[:])
	if _, err := e.strains.Call(ctx, uint64(m.ptr), uint64(e.scorePtr), uint64(buf), uint64(n), uint64(e.secPtr)); err != nil {
		return nil, 0, ErrCalc
	}
	return e.readF64s(buf, int(n)), e.readF64s(e.secPtr, 1)[0], nil
}

// Route says which calculator pays a play.
func Route(mode int, mods uint32) Calculator {
	e, err := get()
	if err != nil {
		return Failed
	}
	e.mu.Lock()
	defer e.mu.Unlock()
	r, _ := e.route.Call(ctx, uint64(mode), uint64(mods))
	return Calculator(int32(r[0]))
}

// Version names the library and the calculators it carries.
func Version() string {
	e, err := get()
	if err != nil {
		return "somtum-pp (failed to load: " + err.Error() + ")"
	}
	e.mu.Lock()
	defer e.mu.Unlock()
	r, _ := e.versionFn.Call(ctx)
	p := uint32(r[0])
	var out []byte
	for {
		b, ok := e.mem.ReadByte(p)
		if !ok || b == 0 {
			break
		}
		out = append(out, b)
		p++
	}
	return string(out)
}

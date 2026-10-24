package payment

import (
	"context"
	"errors"
	"net/http"
	"time"
)

type Handler struct {
	Provider Provider
	Timeout  time.Duration
}

func (h *Handler) Checkout(w http.ResponseWriter, r *http.Request) {
	req, err := decode(r)
	if err != nil {
		writeError(w, http.StatusBadRequest, err)
		return
	}
	if err := ValidateCheckout(req); err != nil {
		writeError(w, http.StatusUnprocessableEntity, err)
		return
	}
	ctx, cancel := context.WithTimeout(r.Context(), h.Timeout)
	defer cancel()
	res, err := h.Provider.Charge(ctx, req)
	if errors.Is(ctx.Err(), context.DeadlineExceeded) {
		err = &ProviderTimeout{OrderID: req.OrderID, After: h.Timeout}
		w.Header().Set("Retry-After", "2")
	}
	if err != nil {
		status := http.StatusBadGateway
		if Retryable(err) {
			status = http.StatusServiceUnavailable
		}
		writeError(w, status, err)
		return
	}
	writeJSON(w, http.StatusCreated, res)
}

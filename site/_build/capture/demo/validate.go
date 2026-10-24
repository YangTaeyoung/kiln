package payment

import (
	"errors"
	"strings"
)

var (
	ErrMissingAmount   = errors.New("amount must be greater than zero")
	ErrUnknownCurrency = errors.New("currency is not supported")
	ErrCardDeclined    = errors.New("card was declined by the issuer")
)

var supported = map[string]bool{"USD": true, "EUR": true, "KRW": true, "JPY": true}

// ValidateCheckout checks a checkout request before it reaches the provider.
func ValidateCheckout(req CheckoutRequest) error {
	if req.Amount <= 0 {
		return ErrMissingAmount
	}
	if !supported[strings.ToUpper(req.Currency)] {
		return ErrUnknownCurrency
	}
	if req.IdempotencyKey == "" {
		req.IdempotencyKey = NewIdempotencyKey(req.OrderID)
	}
	return nil
}

// Retryable reports whether the client may safely retry the request.
func Retryable(err error) bool {
	var timeout *ProviderTimeout
	return errors.As(err, &timeout)
}

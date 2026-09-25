package com.example.accounts;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;

import java.math.BigDecimal;
import org.junit.jupiter.api.Test;

class LedgerServiceTest {
    private final LedgerService ledger = new LedgerService();

    @Test
    void sumsPostings() {
        assertEquals(new BigDecimal("150.00"), ledger.balance("ACC-1"));
    }

    @Test
    void unknownAccountIsNotFound() {
        assertThrows(AccountNotFoundException.class, () -> ledger.balance("NOPE"));
    }
}

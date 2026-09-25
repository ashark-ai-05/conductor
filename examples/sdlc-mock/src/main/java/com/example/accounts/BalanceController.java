package com.example.accounts;

import java.util.Map;
import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.PathVariable;
import org.springframework.web.bind.annotation.RestController;

@RestController
public class BalanceController {
    private final LedgerService ledger;

    public BalanceController(LedgerService ledger) {
        this.ledger = ledger;
    }

    @GetMapping("/accounts/{id}/balance")
    public Map<String, String> balance(@PathVariable String id) {
        return Map.of("accountId", id, "balance", ledger.balance(id).toPlainString());
    }
}

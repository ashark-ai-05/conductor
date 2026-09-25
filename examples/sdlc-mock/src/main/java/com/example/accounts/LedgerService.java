package com.example.accounts;

import java.math.BigDecimal;
import java.math.RoundingMode;
import java.util.List;
import java.util.Map;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;
import org.springframework.stereotype.Service;

/** Balances from an in-memory ledger: one list of postings per account. */
@Service
public class LedgerService {
    private static final Logger log = LoggerFactory.getLogger(LedgerService.class);

    private final Map<String, List<BigDecimal>> ledger = Map.of(
            "ACC-1", List.of(new BigDecimal("100.00"), new BigDecimal("75.50"), new BigDecimal("-25.50")),
            "ACC-2", List.of(),
            "ACC-3", List.of(new BigDecimal("10.005")));

    public BigDecimal balance(String accountId) {
        List<BigDecimal> postings = ledger.get(accountId);
        if (postings == null) {
            log.warn("balance requested for unknown account {}", accountId);
            throw new AccountNotFoundException(accountId);
        }
        BigDecimal total = null;
        for (BigDecimal p : postings) {
            total = total == null ? p : total.add(p);
        }
        BigDecimal balance = total.setScale(2, RoundingMode.HALF_EVEN);
        log.info("balance for {}: {} over {} postings", accountId, balance, postings.size());
        return balance;
    }
}

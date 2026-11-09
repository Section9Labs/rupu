//! Explicit operator-only Board writes. Worker `post/directive/retract` still
//! reject the reserved operator address; no roster identity is forged.
use super::*;

pub enum OperatorBoardWrite {
    Post {
        kind: PostKind,
        to: Option<Address>,
        body: String,
        expected_run_id: Option<String>,
    },
    Directive {
        to: Option<Address>,
        body: String,
        expected_run_id: Option<String>,
    },
    Retract {
        expected: Directive,
    },
}

impl Bus {
    pub fn operator_claim(
        &self,
        key: &str,
        ttl: Duration,
        check_owner: impl FnOnce() -> Result<(), FleetError>,
    ) -> Result<crate::OperatorClaimOutcome, FleetError> {
        if self.participant != address::OPERATOR || self.is_joined() {
            return Err(FleetError::Refused(
                "operator claims require an unjoined operator bus".into(),
            ));
        }
        let _lock = self.lock()?;
        check_owner()?;
        self.board.operator_claim(key, ttl)
    }

    pub fn operator_release(
        &self,
        key: &str,
        token: &str,
        check_owner: impl FnOnce() -> Result<(), FleetError>,
    ) -> Result<bool, FleetError> {
        if self.participant != address::OPERATOR || self.is_joined() {
            return Err(FleetError::Refused(
                "operator release requires an unjoined operator bus".into(),
            ));
        }
        let _lock = self.lock()?;
        check_owner()?;
        self.board.operator_release(key, token)
    }

    /// Actor attribution is supplied by the trusted operator surface, not by
    /// action JSON. `check_owner` runs under the message log lock before any
    /// append. A partial apply retains the normal PartlyDelivered receipt/error.
    pub fn operator_board(
        &self,
        action: OperatorBoardWrite,
        via: OperatorVia,
        client: Option<String>,
        check_owner: impl FnOnce() -> Result<(), FleetError>,
    ) -> Result<SendReceipt, BusError> {
        if self.participant != address::OPERATOR || self.is_joined() {
            return Err(FleetError::Refused(
                "operator Board writes require an unjoined operator bus".into(),
            ));
        }
        match action {
            OperatorBoardWrite::Post {
                kind,
                to,
                body,
                expected_run_id,
            } => {
                check_text(&body)?;
                check_recipient(to.as_ref(), expected_run_id.as_deref())?;
                let to = to.as_ref().filter(|a| **a != Address::Broadcast);
                self.write_as_checked(
                    address::OPERATOR,
                    Draft {
                        kind: MessageKind::Post,
                        to,
                        post_kind: Some(kind),
                        body: &body,
                        expected_run_id: expected_run_id.as_deref(),
                        via: Some(via),
                        client,
                        ..Draft::plain()
                    },
                    check_owner,
                    |m| {
                        self.board.post(&BoardPost {
                            id: Some(m.id.clone()),
                            author: m.from.clone(),
                            ts: m.ts.clone(),
                            kind,
                            body: m.body.clone(),
                            addressed_to: m.to.clone(),
                        })?;
                        if m.to.is_some() {
                            self.deliver(m, false)?;
                        }
                        Ok(())
                    },
                )
            }
            OperatorBoardWrite::Directive {
                to,
                body,
                expected_run_id,
            } => {
                check_text(&body)?;
                check_recipient(to.as_ref(), expected_run_id.as_deref())?;
                let to = to.as_ref().filter(|a| **a != Address::Broadcast);
                self.write_as_checked(
                    address::OPERATOR,
                    Draft {
                        kind: MessageKind::Directive,
                        to,
                        body: &body,
                        expected_run_id: expected_run_id.as_deref(),
                        via: Some(via),
                        client,
                        ..Draft::plain()
                    },
                    check_owner,
                    |m| {
                        self.board
                            .put_directive(&Directive {
                                id: m.id.clone(),
                                author: m.from.clone(),
                                ts: m.ts.clone(),
                                body: m.body.clone(),
                                addressed_to: m.to.clone(),
                            })
                            .map(drop)
                    },
                )
            }
            OperatorBoardWrite::Retract { expected } => {
                if expected.id.is_empty() || expected.id.len() > 128 {
                    return Err(FleetError::Refused("invalid directive id".into()));
                }
                self.write_as_checked(
                    address::OPERATOR,
                    Draft {
                        kind: MessageKind::Retract,
                        refs: Refs {
                            retracts: Some(expected.id.clone()),
                            reply_to: None,
                        },
                        via: Some(via),
                        client,
                        ..Draft::plain()
                    },
                    || {
                        check_owner()?;
                        if !self
                            .board
                            .operator_directives()?
                            .iter()
                            .any(|d| d == &expected)
                        {
                            return Err(FleetError::Refused(
                                "directive changed or is no longer live; refresh before retracting"
                                    .into(),
                            ));
                        }
                        Ok(())
                    },
                    |_| self.board.retract_directive(&expected.id),
                )
            }
        }
    }
}
fn check_text(body: &str) -> Result<(), FleetError> {
    if body.trim().is_empty() || body.len() > 65_536 {
        return Err(FleetError::Refused(
            "Board text must contain 1..65536 bytes".into(),
        ));
    }
    Ok(())
}
fn check_recipient(to: Option<&Address>, expected: Option<&str>) -> Result<(), FleetError> {
    if to == Some(&Address::Operator) || to == Some(&Address::Parent) {
        return Err(FleetError::Refused(
            "operator Board writes cannot address operator or parent".into(),
        ));
    }
    if expected.is_some() && !matches!(to, Some(Address::Named(_))) {
        return Err(FleetError::RecipientChanged);
    }
    Ok(())
}

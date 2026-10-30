/**
 * Balance page actions: short always-visible lines. The full rules they
 * summarize stay in their original keys and open from an ⓘ beside them.
 */
export const walletActionsMessagesEn = {
  "common.moreInfo": "More information",

  "marketWallet.stat.held": "Set aside",
  "marketWallet.stat.owed": "Due",
  "marketWallet.stat.transferable": "Sendable",
  "marketWallet.stat.heldHelp": "Reserved for open orders, C2C sales and disputes. It returns to your available balance when they settle.",
  "marketWallet.stat.owedHelp": "Charges already due for services that are running. They come out of the available balance.",
  "marketWallet.stat.transferableHelp": "What you can send or sell now: available balance minus amounts due, within the send limits.",
  "marketWallet.amountRangeDetail": "This is the balance you receive. The maintenance fee is added on top and does not count toward this range.",
  "walletPolicy.disclosureShort": "Balance can't be cashed out.",

  "walletTransfer.helpShort": "Free and instant within this Router · can't be undone",
  "walletTransfer.allShort": "All",
  "walletTransfer.availableShort": "Sendable {amount}",
  "walletTransfer.finalShort": "Sends can't be undone.",

  "c2c.tab.buy": "Buy",
  "c2c.tab.sell": "Sell",
  "c2c.tab.orders": "My orders",
  "c2c.activeBanner": "C2C orders in progress: {count}",
  "c2c.activeTitle": "Active C2C orders: {count}",
  "c2c.viewOrders": "View",
  "c2c.sellers": "Sellers",
  "c2c.introShort": "Buy balance from other users and pay them directly.",
  "c2c.payWithin": "Pay within {minutes} min",
  "c2c.paymentNotes": "Payment notes",
  "c2c.ackShort": "I'll pay only after the order shows an active payment window.",
  "c2c.paidAckShort": "I have paid the exact amount to the account shown.",
  "c2c.releaseAckShort": "I personally verified the payment arrived. Release can't be undone.",
  "c2c.sellerAckShort": "Orders are accepted automatically while selling, and I will verify receipt before releasing credit.",
  "c2c.sellIntro": "Sell spare balance to other users. Buyers pay you directly.",
  "c2c.setUpSelling": "Set up selling",
  "c2c.descriptionHelpShort": "How buyers pay you and reach you.",
} as const;

export const walletActionsMessagesZh: Record<keyof typeof walletActionsMessagesEn, string> = {
  "common.moreInfo": "查看说明",

  "marketWallet.stat.held": "暂扣",
  "marketWallet.stat.owed": "待扣",
  "marketWallet.stat.transferable": "可转出",
  "marketWallet.stat.heldHelp": "为进行中的订单、C2C 出售和争议暂时冻结，结束后自动回到可用余额。",
  "marketWallet.stat.owedHelp": "运行中服务已产生、尚未结算的费用，会从可用余额中扣除。",
  "marketWallet.stat.transferableHelp": "当前可转出或用于 C2C 出售的金额：可用余额扣除待扣费用，并受转出限额约束。",
  "marketWallet.amountRangeDetail": "这是实际到账的余额。维护费另计，不计入此范围。",
  "walletPolicy.disclosureShort": "余额不可提现。",

  "walletTransfer.helpShort": "站内免费即时到账 · 转出后无法撤回",
  "walletTransfer.allShort": "全部",
  "walletTransfer.availableShort": "可转出 {amount}",
  "walletTransfer.finalShort": "转出后无法撤回。",

  "c2c.tab.buy": "购入",
  "c2c.tab.sell": "出售",
  "c2c.tab.orders": "我的订单",
  "c2c.activeBanner": "你有 {count} 笔进行中的 C2C 订单",
  "c2c.activeTitle": "{count} 笔进行中的 C2C 订单",
  "c2c.viewOrders": "查看",
  "c2c.sellers": "卖家",
  "c2c.introShort": "向其他用户购入余额，直接向卖家付款。",
  "c2c.payWithin": "{minutes} 分钟内付款",
  "c2c.paymentNotes": "付款说明",
  "c2c.ackShort": "订单显示可付款后，我才会付款。",
  "c2c.paidAckShort": "我已向显示的收款账户支付准确金额。",
  "c2c.releaseAckShort": "我已亲自核实到账，放行后无法撤回。",
  "c2c.sellerAckShort": "出售期间自动接单；我会核实到账后再放行。",
  "c2c.sellIntro": "把闲置余额卖给其他用户，买家直接付款给你。",
  "c2c.setUpSelling": "设置出售",
  "c2c.descriptionHelpShort": "买家如何付款、如何联系你。",
};

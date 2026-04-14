#!/usr/bin/env python
#  -*- coding: utf-8 -*-
__author__ = 'limin'

'''
双均线策略
注: 该示例策略仅用于功能示范, 实盘时请根据自己的策略/经验进行修改
'''
from datetime import date, datetime
import os

from tqsdk import BacktestFinished, TargetPosTask, TqApi, TqAuth, TqBacktest
from tqsdk.tafunc import ma

SHORT = 30  # 短周期
LONG = 60  # 长周期
DEFAULT_SYMBOL = "SHFE.bu2012"  # 合约代码
DEFAULT_START_DATE = date(2020, 9, 1)
DEFAULT_END_DATE = date(2020, 11, 30)


def parse_env_date(name, default):
    raw = os.getenv(name)
    return datetime.strptime(raw, "%Y-%m-%d").date() if raw else default


SYMBOL = os.getenv("TQ_TEST_SYMBOL", DEFAULT_SYMBOL)
START_DATE = parse_env_date("TQ_START_DT", DEFAULT_START_DATE)
END_DATE = parse_env_date("TQ_END_DT", DEFAULT_END_DATE)

auth_user = os.getenv("TQ_AUTH_USER", "")
auth_pass = os.getenv("TQ_AUTH_PASS", "")

api = TqApi(
    backtest=TqBacktest(start_dt=START_DATE, end_dt=END_DATE),
    auth=TqAuth(auth_user, auth_pass),
)
print("策略开始运行")

data_length = LONG + 2  # k线数据长度
# "duration_seconds=60"为一分钟线, 日线的duration_seconds参数为: 24*60*60
klines = api.get_kline_serial(SYMBOL, duration_seconds=60, data_length=data_length)
target_pos = TargetPosTask(api, SYMBOL)

try:
    while True:
        api.wait_update()

        if api.is_changing(klines.iloc[-1], "datetime"):  # 产生新k线:重新计算SMA
            short_avg = ma(klines["close"], SHORT)  # 短周期
            long_avg = ma(klines["close"], LONG)  # 长周期

            # 均线下穿，做空
            if long_avg.iloc[-2] < short_avg.iloc[-2] and long_avg.iloc[-1] > short_avg.iloc[-1]:
                target_pos.set_target_volume(-3)
                print("均线下穿，做空")

            # 均线上穿，做多
            if short_avg.iloc[-2] < long_avg.iloc[-2] and short_avg.iloc[-1] > long_avg.iloc[-1]:
                target_pos.set_target_volume(3)
                print("均线上穿，做多")
except BacktestFinished:
    print("回测结束")
    api.close()

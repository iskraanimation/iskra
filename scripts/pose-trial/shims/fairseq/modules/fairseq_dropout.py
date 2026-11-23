import torch.nn as nn


class FairseqDropout(nn.Dropout):
    def __init__(self, p, module_name=None):
        super().__init__(p)

import torch.nn.functional as F


def softmax(x, dim, onnx_trace=False):
    return F.softmax(x.float(), dim=dim)
